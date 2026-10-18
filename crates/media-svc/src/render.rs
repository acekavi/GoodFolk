//! Invoice PDFs. `POST /render` takes an `invoice-render` push, lays its [`InvoiceData`] out with the bundled Typst
//! template and fonts, stores the PDF privately and publishes [`MediaResult::InvoiceRendered`].
//!
//! Typst runs in-process (on a blocking thread), not in a worker subprocess: it is pure Rust and the template is our
//! own. The answer is 200 only once the result is published, 400 for a body that is not a job or a job too large to
//! render, and 503 otherwise, so the bus delivers the job again. The key is deterministic, so a redelivered job
//! rewrites the same object.

use std::sync::{Arc, LazyLock};

use anyhow::Context;
use axum::body::Bytes as Body;
use axum::extract::State;
use axum::http::StatusCode;
use events::PushEnvelope;
use media_types::{
    InvoiceData, InvoiceRenderJob, MAX_INVOICE_LINES, MAX_INVOICE_PAYMENTS, MAX_INVOICE_TAXES, MediaResult,
};
use storage::{Bucket, invoice_pdf_key};
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
/// The largest `/render` body: a push of an invoice at every limit ([`MAX_INVOICE_LINES`] lines of 200 four-byte
/// characters, base64-encoded in the envelope) is about 3 MiB.
pub const MAX_RENDER_BODY: usize = 8 * 1024 * 1024;
/// Compilations a memoised result survives unused before it is evicted, so the cache does not grow with every
/// invoice while the template's own parse stays warm.
const CACHE_AGE: usize = 10;

const TEMPLATE: &str = include_str!("../templates/invoice.typ");
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
    let options = typst_pdf::PdfOptions { ident: Smart::Custom(data.serial.clone()), ..Default::default() };
    typst_pdf::pdf(&document, &options).map_err(|errors| failed("exporting", &errors))
}

/// `data` laid out by the template, before it is exported.
pub fn layout(data: &InvoiceData) -> anyhow::Result<PagedDocument> {
    let world = InvoiceWorld { data: Bytes::new(serde_json::to_vec(data).context("serializing the invoice data")?) };
    let compiled = typst::compile::<PagedDocument>(&world);
    typst::comemo::evict(CACHE_AGE);
    compiled.output.map_err(|errors| failed("laying out", &errors))
}

fn failed(stage: &str, errors: &EcoVec<SourceDiagnostic>) -> anyhow::Error {
    let messages: Vec<&str> = errors.iter().map(|error| error.message.as_str()).collect();
    anyhow::anyhow!("{stage} the invoice failed: {}", messages.join("; "))
}

/// What every render shares, made once: the standard library, the fonts and the parsed template.
struct Assets {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
    data: FileId,
}

static ASSETS: LazyLock<Assets> = LazyLock::new(|| {
    let fonts: Vec<Font> = FONTS.into_iter().flat_map(|font| Font::iter(Bytes::new(font))).collect();
    let file = |path: &str| {
        RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).expect("a valid virtual path")).intern()
    };
    Assets {
        library: LazyHash::new(Library::default()),
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
        main: Source::new(file("/invoice.typ"), TEMPLATE.to_owned()),
        data: file("/data.json"),
    }
});

/// The template as the main file and the invoice as `data.json`; nothing else is readable.
struct InvoiceWorld {
    data: Bytes,
}

impl World for InvoiceWorld {
    fn library(&self) -> &LazyHash<Library> {
        &ASSETS.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &ASSETS.book
    }

    fn main(&self) -> FileId {
        ASSETS.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == ASSETS.main.id() { Ok(ASSETS.main.clone()) } else { Err(not_found(id)) }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == ASSETS.data {
            Ok(self.data.clone())
        } else if id == ASSETS.main.id() {
            Ok(Bytes::from_string(ASSETS.main.clone()))
        } else {
            Err(not_found(id))
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        ASSETS.fonts.get(index).cloned()
    }

    /// None: the template prints the invoice's own dates, never today's.
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

fn not_found(id: FileId) -> FileError {
    FileError::NotFound(id.vpath().get_without_slash().into())
}
