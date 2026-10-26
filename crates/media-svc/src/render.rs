//! Invoice and report PDFs. `POST /render` takes an `invoice-render` push and lays its [`InvoiceData`] out with the
//! bundled invoice template; `POST /render-report` takes a `report-render` push and lays its data out with the report
//! template it names. Both store the PDF privately and publish [`MediaResult::InvoiceRendered`] or
//! [`MediaResult::ReportRendered`].
//!
//! Typst runs in-process (on a blocking thread), not in a worker subprocess: it is pure Rust and the templates are our
//! own. The answer is 200 only once the result is published, 400 for a body that is not a job (and an invoice job too
//! large to render), and 503 otherwise, so the bus delivers the job again. A report job that can never render (its
//! data over [`MAX_REPORT_BYTES`], or refused by its template) publishes [`MediaResult::ReportFailed`] instead, so the
//! report is marked failed. The keys are deterministic, so a redelivered job rewrites the same object.

use std::sync::{Arc, LazyLock};

use anyhow::Context;
use axum::body::Bytes as Body;
use axum::extract::State;
use axum::http::StatusCode;
use events::PushEnvelope;
use media_types::{
    InvoiceData, InvoiceRenderJob, MAX_INVOICE_LINES, MAX_INVOICE_PAYMENTS, MAX_INVOICE_TAXES, MAX_REPORT_BYTES,
    MediaResult, ReportRenderJob, ReportTemplate,
};
use storage::{Bucket, invoice_pdf_key, report_pdf_key};
use tokio::sync::Semaphore;
use typst::diag::{FileError, FileResult, SourceDiagnostic};
use typst::ecow::EcoVec;
use typst::foundations::{Bytes, Datetime, Duration, Smart};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;
use uuid::Uuid;

use crate::MediaSvc;
use crate::work::publish;

/// Renders at once; more wait for a permit.
pub(crate) const CONCURRENT_RENDERS: usize = 2;
/// The largest `/render` and `/render-report` body: a push of an invoice at every limit ([`MAX_INVOICE_LINES`] lines of
/// 200 four-byte characters, base64-encoded in the envelope) is about 3 MiB, and one of a report's data at
/// [`MAX_REPORT_BYTES`] under 5.5 MiB.
pub const MAX_RENDER_BODY: usize = 8 * 1024 * 1024;
/// Compilations a memoised result survives unused before it is evicted, so the cache does not grow with every
/// invoice while the templates' own parse stays warm.
const CACHE_AGE: usize = 10;

const TEMPLATE: &str = include_str!("../templates/invoice.typ");
/// What every report template imports: the page, the fonts and the head.
const REPORT_COMMON: &str = include_str!("../templates/reports/common.typ");
/// Each report template, under its name.
const REPORT_TEMPLATES: [(ReportTemplate, &str); 6] = [
    (ReportTemplate::Summary, include_str!("../templates/reports/summary.typ")),
    (ReportTemplate::DailyList, include_str!("../templates/reports/daily_list.typ")),
    (ReportTemplate::TdlFormA, include_str!("../templates/reports/tdl_form_a.typ")),
    (ReportTemplate::Sscl, include_str!("../templates/reports/sscl.typ")),
    (ReportTemplate::Vat, include_str!("../templates/reports/vat.typ")),
    (ReportTemplate::ServiceCharge, include_str!("../templates/reports/service_charge.typ")),
];
const FONTS: [&[u8]; 2] =
    [include_bytes!("../fonts/NotoSans-Regular.ttf"), include_bytes!("../fonts/NotoSans-Bold.ttf")];

#[derive(Clone)]
pub(crate) struct RenderState {
    pub svc: MediaSvc,
    pub permits: Arc<Semaphore>,
}

pub(crate) async fn render(State(state): State<RenderState>, body: Body) -> StatusCode {
    let Ok(envelope) = serde_json::from_slice::<PushEnvelope>(&body) else {
        tracing::error!("an invoice-render push is not a push envelope");
        return StatusCode::BAD_REQUEST;
    };
    let job: InvoiceRenderJob = match envelope.decode() {
        Ok(job) => job,
        Err(error) => {
            tracing::error!(%error, id = envelope.dedupe_id(), "an invoice-render message is not a job");
            return StatusCode::BAD_REQUEST;
        }
    };
    if let Some(what) = oversized(&job.data) {
        tracing::error!(invoice = %job.invoice_id, what, "an invoice-render job is too large to render");
        return StatusCode::BAD_REQUEST;
    }
    match handle(&state, job.tenant_id, job.invoice_id, job.data).await {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(invoice = %job.invoice_id, error = format!("{error:#}"), "the job will be delivered again");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

async fn handle(state: &RenderState, tenant_id: Uuid, invoice_id: Uuid, data: InvoiceData) -> anyhow::Result<()> {
    // The permit moves into the render, so it is released when the render ends, even if this request is dropped.
    let permit = state.permits.clone().acquire_owned().await?;
    let pdf = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        pdf(&data)
    })
    .await??;
    let key = invoice_pdf_key(tenant_id, invoice_id);
    state.svc.storage.put(Bucket::Private, &key, pdf, "application/pdf", None).await?;
    publish(&state.svc, MediaResult::InvoiceRendered { tenant_id, invoice_id, key }).await
}

pub(crate) async fn render_report(State(state): State<RenderState>, body: Body) -> StatusCode {
    let Ok(envelope) = serde_json::from_slice::<PushEnvelope>(&body) else {
        tracing::error!("a report-render push is not a push envelope");
        return StatusCode::BAD_REQUEST;
    };
    let job: ReportRenderJob = match envelope.decode() {
        Ok(job) => job,
        Err(error) => {
            tracing::error!(%error, id = envelope.dedupe_id(), "a report-render message is not a job");
            return StatusCode::BAD_REQUEST;
        }
    };
    let report = job.report_id;
    match handle_report(&state, job).await {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(%report, error = format!("{error:#}"), "the job will be delivered again");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// Renders, stores and publishes the job's PDF, or publishes why it can never be rendered. An error is worth
/// delivering the job again for.
async fn handle_report(state: &RenderState, job: ReportRenderJob) -> anyhow::Result<()> {
    let ReportRenderJob { tenant_id, report_id, template, data } = job;
    let data = serde_json::to_vec(&data)?;
    let rendered = if data.len() > MAX_REPORT_BYTES {
        Err(anyhow::anyhow!("the report data is too large to render (over {MAX_REPORT_BYTES} bytes)"))
    } else {
        let permit = state.permits.clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            report_pdf_of(template, data)
        })
        .await?
    };
    let result = match rendered {
        Ok(pdf) => {
            let key = report_pdf_key(tenant_id, report_id);
            state.svc.storage.put(Bucket::Private, &key, pdf, "application/pdf", None).await?;
            MediaResult::ReportRendered { tenant_id, report_id, key }
        }
        Err(error) => {
            tracing::error!(report = %report_id, template = template.as_str(), error = format!("{error:#}"),
                "a report can never be rendered");
            MediaResult::ReportFailed { tenant_id, report_id, reason: format!("{error:#}") }
        }
    };
    publish(&state.svc, result).await
}

/// What of `data` is over its limit, if anything.
fn oversized(data: &InvoiceData) -> Option<&'static str> {
    if data.lines.len() > MAX_INVOICE_LINES {
        Some("lines")
    } else if data.payments.len() > MAX_INVOICE_PAYMENTS {
        Some("payments")
    } else if data.taxes.len() > MAX_INVOICE_TAXES {
        Some("taxes")
    } else {
        None
    }
}

/// `data` as a PDF. The same data always gives the same bytes.
pub fn pdf(data: &InvoiceData) -> anyhow::Result<Vec<u8>> {
    let document = layout(data)?;
    export(&document, Smart::Custom(data.serial.clone()), "invoice")
}

/// `data` laid out by the template, before it is exported.
pub fn layout(data: &InvoiceData) -> anyhow::Result<PagedDocument> {
    let data = serde_json::to_vec(data).context("serializing the invoice data")?;
    compile(ASSETS.invoice, data, "invoice")
}

/// A report's `data` laid out with `template` as a PDF. The same data always gives the same bytes.
pub fn report_pdf(template: ReportTemplate, data: &serde_json::Value) -> anyhow::Result<Vec<u8>> {
    report_pdf_of(template, serde_json::to_vec(data).context("serializing the report data")?)
}

fn report_pdf_of(template: ReportTemplate, data: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let document = compile(ASSETS.report(template), data, "report")?;
    export(&document, Smart::Auto, "report")
}

/// A report's `data` laid out by `template`, before it is exported.
pub fn report_layout(template: ReportTemplate, data: &serde_json::Value) -> anyhow::Result<PagedDocument> {
    let data = serde_json::to_vec(data).context("serializing the report data")?;
    compile(ASSETS.report(template), data, "report")
}

/// `main` compiled with `data` as `/data.json`.
fn compile(main: FileId, data: Vec<u8>, what: &str) -> anyhow::Result<PagedDocument> {
    let world = TemplateWorld { main, data: Bytes::new(data) };
    let compiled = typst::compile::<PagedDocument>(&world);
    typst::comemo::evict(CACHE_AGE);
    compiled.output.map_err(|errors| failed("laying out", what, &errors))
}

fn export(document: &PagedDocument, ident: Smart<String>, what: &str) -> anyhow::Result<Vec<u8>> {
    let options = typst_pdf::PdfOptions { ident, ..Default::default() };
    typst_pdf::pdf(document, &options).map_err(|errors| failed("exporting", what, &errors))
}

fn failed(stage: &str, what: &str, errors: &EcoVec<SourceDiagnostic>) -> anyhow::Error {
    let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
    anyhow::anyhow!("{stage} the {what} failed: {}", messages.join("; "))
}

/// What every render shares, made once: the standard library, the fonts and the parsed templates.
struct Assets {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    /// Every template: the invoice's, the reports' shared one and each report's.
    sources: Vec<Source>,
    invoice: FileId,
    reports: Vec<(ReportTemplate, FileId)>,
    data: FileId,
}

impl Assets {
    fn report(&self, template: ReportTemplate) -> FileId {
        self.reports.iter().find(|(t, _)| *t == template).map(|(_, id)| *id).expect("every report has a template")
    }
}

static ASSETS: LazyLock<Assets> = LazyLock::new(|| {
    let fonts: Vec<Font> = FONTS.into_iter().flat_map(|font| Font::iter(Bytes::new(font))).collect();
    let file = |path: &str| {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).expect("a valid virtual path")).intern()
    };
    let invoice = Source::new(file("/invoice.typ"), TEMPLATE.to_owned());
    let common = Source::new(file("/reports/common.typ"), REPORT_COMMON.to_owned());
    let reports: Vec<Source> = REPORT_TEMPLATES
        .iter()
        .map(|(template, text)| Source::new(file(&format!("/reports/{}.typ", template.as_str())), (*text).to_owned()))
        .collect();
    Assets {
        library: LazyHash::new(Library::default()),
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
        invoice: invoice.id(),
        reports: REPORT_TEMPLATES
            .iter()
            .zip(&reports)
            .map(|((template, _), source)| (*template, source.id()))
            .collect(),
        sources: [invoice, common].into_iter().chain(reports).collect(),
        data: file("/data.json"),
    }
});

/// One template as the main file and the data as `/data.json`; only the templates and the data are readable.
struct TemplateWorld {
    main: FileId,
    data: Bytes,
}

impl TemplateWorld {
    fn template(&self, id: FileId) -> FileResult<Source> {
        ASSETS.sources.iter().find(|source| source.id() == id).cloned().ok_or_else(|| not_found(id))
    }
}

impl World for TemplateWorld {
    fn library(&self) -> &LazyHash<Library> {
        &ASSETS.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &ASSETS.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.template(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == ASSETS.data { Ok(self.data.clone()) } else { self.template(id).map(Bytes::from_string) }
    }

    fn font(&self, index: usize) -> Option<Font> {
        ASSETS.fonts.get(index).cloned()
    }

    /// None: the templates print the data's own dates, never today's.
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

fn not_found(id: FileId) -> FileError {
    FileError::NotFound(id.vpath().get_without_slash().into())
}
