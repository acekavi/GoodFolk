mod common;

use std::collections::BTreeMap;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{CaptureBus, SECRET, Server, http, start, start_with};
use events::{PushEnvelope, PushMessage};
use media_svc::render;
use media_types::{
    InvoiceData, InvoiceKind, InvoiceLine, InvoicePayment, InvoiceRenderJob, InvoiceTax, MediaResult, Purchaser,
    ReportRenderJob, ReportTemplate, Supplier,
};
use serde_json::{Value, json};
use storage::{Bucket, invoice_pdf_key, report_pdf_key};
use time::macros::date;
use typst::layout::{Frame, FrameItem};
use typst_layout::PagedDocument;
use uuid::Uuid;

/// A VAT-registered hotel's tax invoice: a room charged in USD and a restaurant bill in LKR.
fn tax_invoice() -> InvoiceData {
    InvoiceData {
        kind: InvoiceKind::Invoice,
        supplier: Supplier {
            legal_name: "Galle Fort Hotels (Pvt) Ltd".into(),
            address: Some("28 Church Street\nGalle Fort, Galle 80000".into()),
            tin: Some("114567890".into()),
            sltda_licence: Some("SLTDA/GAL/0042".into()),
        },
        purchaser: Purchaser { name: "Acme Travels (Pvt) Ltd".into(), tin: Some("102345678".into()) },
        serial: "GAL-2026-000042".into(),
        title: "TAX INVOICE".into(),
        issued_on: date!(2026 - 10 - 17),
        supply_from: date!(2026 - 10 - 15),
        supply_to: date!(2026 - 10 - 17),
        lines: vec![
            InvoiceLine {
                description: "Deluxe double, 2 nights".into(),
                quantity: 2,
                currency: "USD".into(),
                total: 20_000,
                total_lkr: 60_000,
            },
            InvoiceLine {
                description: "Restaurant".into(),
                quantity: 1,
                currency: "LKR".into(),
                total: 1_342_545,
                total_lkr: 13_425,
            },
        ],
        taxes: vec![
            InvoiceTax { code: "SC".into(), name: "Service charge".into(), rate_bp: 1000, amount_lkr: 1_000 },
            InvoiceTax { code: "TDL".into(), name: "Tourism development levy".into(), rate_bp: 100, amount_lkr: 700 },
            InvoiceTax { code: "SSCL".into(), name: "SSCL".into(), rate_bp: 250, amount_lkr: 1_943 },
            InvoiceTax { code: "VAT".into(), name: "VAT".into(), rate_bp: 1800, amount_lkr: 14_341 },
        ],
        net_lkr: 55_441,
        total_lkr: 73_425,
        total_words: "Seventy-Three Thousand Four Hundred Twenty-Five Rupees Only".into(),
        payments: vec![
            InvoicePayment { method: "card_terminal".into(), amount: 20_000, currency: "USD".into(), amount_lkr: None },
            // Printed as given: the template never rounds.
            InvoicePayment {
                method: "cash".into(),
                amount: 1_342_500,
                currency: "LKR".into(),
                amount_lkr: Some(13_425),
            },
        ],
        vat_registered: true,
        credits_serial: None,
        reason: None,
    }
}

fn credit_note() -> InvoiceData {
    let invoice = tax_invoice();
    InvoiceData {
        kind: InvoiceKind::CreditNote,
        serial: "CN-GAL-2026-000001".into(),
        title: "CREDIT NOTE".into(),
        lines: vec![InvoiceLine {
            description: "Restaurant".into(),
            quantity: -1,
            currency: "LKR".into(),
            total: -1_342_545,
            total_lkr: -13_425,
        }],
        taxes: vec![InvoiceTax { code: "VAT".into(), name: "VAT".into(), rate_bp: 1800, amount_lkr: -2_048 }],
        net_lkr: -11_377,
        total_lkr: -13_425,
        total_words: "Minus Thirteen Thousand Four Hundred Twenty-Five Rupees Only".into(),
        payments: vec![],
        credits_serial: Some(invoice.serial.clone()),
        reason: Some("Billed to the wrong account".into()),
        ..invoice
    }
}

/// The laid-out text of every page, one space between runs. The PDF's own streams are compressed, so the text is
/// read from Typst's document before export.
fn text(data: &InvoiceData) -> String {
    text_of(&render::layout(data).unwrap())
}

fn text_of(document: &PagedDocument) -> String {
    fn walk(frame: &Frame, out: &mut Vec<String>) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => walk(&group.frame, out),
                FrameItem::Text(text) => out.push(text.text.to_string()),
                _ => {}
            }
        }
    }
    let mut runs = Vec::new();
    for page in document.pages() {
        walk(&page.frame, &mut runs);
    }
    runs.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn a_tax_invoice_renders_with_its_title_serial_and_figures() {
    let pdf = render::pdf(&tax_invoice()).unwrap();
    assert!(pdf.starts_with(b"%PDF"), "a PDF");

    let text = text(&tax_invoice());
    for expected in [
        "TAX INVOICE",
        "GAL-2026-000042",
        "Galle Fort Hotels (Pvt) Ltd",
        "TIN: 114567890",
        "SLTDA licence: SLTDA/GAL/0042",
        "Acme Travels (Pvt) Ltd",
        "TIN: 102345678",
        "17 Oct 2026",
        "15 Oct 2026 – 17 Oct 2026",
        "USD 200.00",
        "60,000",
        "13,425",
        "VAT 18%",
        "SSCL 2.5%",
        "Tourism development levy 1%",
        "55,441",
        "73,425",
        "Seventy-Three Thousand Four Hundred Twenty-Five Rupees Only",
        "Card USD 200.00",
        "Cash LKR 13,425",
    ] {
        assert!(text.contains(expected), "{expected:?} is not in {text:?}");
    }
    assert!(!text.contains("Credits invoice"), "an invoice credits nothing");
}

#[test]
fn a_credit_note_names_the_invoice_it_credits_and_prints_negative_amounts() {
    let pdf = render::pdf(&credit_note()).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let text = text(&credit_note());
    for expected in [
        "CREDIT NOTE",
        "CN-GAL-2026-000001",
        "Credits invoice GAL-2026-000042",
        "Reason: Billed to the wrong account",
        "Restaurant -1 -13,425",
        "-2,048",
        "-11,377",
    ] {
        assert!(text.contains(expected), "{expected:?} is not in {text:?}");
    }
}

#[test]
fn rendering_is_fast_and_deterministic() {
    let started = Instant::now();
    let first = render::pdf(&tax_invoice()).unwrap();
    let cold = started.elapsed();
    let started = Instant::now();
    let second = render::pdf(&credit_note()).unwrap();
    let warm = started.elapsed();
    println!("invoice render: {cold:?} cold, {warm:?} warm ({} and {} bytes)", first.len(), second.len());
    if !cfg!(debug_assertions) {
        assert!(cold.as_secs_f64() < 1.0, "a cold render took {cold:?}");
        assert!(warm.as_secs_f64() < 1.0, "a warm render took {warm:?}");
    }
    assert_eq!(render::pdf(&tax_invoice()).unwrap(), first, "the same data gives the same bytes");
}

fn envelope(payload: &serde_json::Value) -> PushEnvelope {
    envelope_on("invoice-render", payload)
}

fn envelope_on(topic: &str, payload: &serde_json::Value) -> PushEnvelope {
    let id = Uuid::now_v7().to_string();
    PushEnvelope {
        message: PushMessage {
            data: STANDARD.encode(serde_json::to_vec(payload).unwrap()),
            message_id: id.clone(),
            attributes: BTreeMap::from([("message_id".to_owned(), id)]),
        },
        subscription: format!("push/{topic}"),
    }
}

async fn post_render(server: &Server, body: Vec<u8>, auth: Option<&str>) -> reqwest::StatusCode {
    let mut request =
        http().post(format!("http://{}/render", server.addr)).header("content-type", "application/json").body(body);
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    request.send().await.unwrap().status()
}

fn job() -> InvoiceRenderJob {
    InvoiceRenderJob { tenant_id: Uuid::now_v7(), invoice_id: Uuid::now_v7(), data: tax_invoice() }
}

fn push(job: &InvoiceRenderJob) -> Vec<u8> {
    serde_json::to_vec(&envelope(&serde_json::to_value(job).unwrap())).unwrap()
}

fn bearer() -> String {
    format!("Bearer {SECRET}")
}

#[tokio::test]
async fn render_rejects_unauthenticated() {
    let server = start().await;
    let job = job();
    assert_eq!(post_render(&server, push(&job), None).await, 401);
    assert_eq!(post_render(&server, push(&job), Some("Bearer not-the-secret-not-the-secret-0000")).await, 401);
    assert!(server.bus.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_repeated_push_stores_and_publishes_the_same_key() {
    let server = start().await;
    let job = job();
    let key = invoice_pdf_key(job.tenant_id, job.invoice_id);
    assert_eq!(key, format!("t/{}/invoices/{}.pdf", job.tenant_id, job.invoice_id));

    assert_eq!(post_render(&server, push(&job), Some(&bearer())).await, 200);
    assert_eq!(post_render(&server, push(&job), Some(&bearer())).await, 200, "a redelivery overwrites");

    let sent = server.bus.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "each delivery publishes");
    let expected =
        MediaResult::InvoiceRendered { tenant_id: job.tenant_id, invoice_id: job.invoice_id, key: key.clone() };
    for message in &sent {
        assert_eq!(message.topic, "media-results");
        assert_eq!(serde_json::from_value::<MediaResult>(message.payload.clone()).unwrap(), expected);
    }
    let stored = server.storage.get(Bucket::Private, &key).await.unwrap();
    assert_eq!(stored, render::pdf(&job.data).unwrap());
    assert_eq!(server.storage.head(Bucket::Public, &key).await.unwrap(), None, "an invoice is never public");
    server.storage.delete(Bucket::Private, &key).await.unwrap();
}

#[tokio::test]
async fn a_malformed_push_is_refused_for_good() {
    let server = start().await;
    assert_eq!(post_render(&server, b"not json".to_vec(), Some(&bearer())).await, 400);
    assert_eq!(post_render(&server, b"{\"message\": 1}".to_vec(), Some(&bearer())).await, 400);
    let not_a_job = serde_json::json!({ "tenant_id": Uuid::now_v7(), "invoice_id": Uuid::now_v7() });
    let body = serde_json::to_vec(&envelope(&not_a_job)).unwrap();
    assert_eq!(post_render(&server, body, Some(&bearer())).await, 400);
    assert!(server.bus.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn render_answers_503_until_the_result_is_published() {
    let server = start_with(CaptureBus { fail: true, ..CaptureBus::default() }).await;
    let job = job();
    assert_eq!(post_render(&server, push(&job), Some(&bearer())).await, 503);
    server.storage.delete(Bucket::Private, &invoice_pdf_key(job.tenant_id, job.invoice_id)).await.unwrap();
}

#[tokio::test]
async fn an_oversized_job_is_refused_for_good() {
    let server = start().await;
    let line = tax_invoice().lines[0].clone();
    let payment = tax_invoice().payments[0].clone();
    let tax = tax_invoice().taxes[0].clone();
    let oversized = [
        InvoiceData { lines: vec![line; media_types::MAX_INVOICE_LINES + 1], ..tax_invoice() },
        InvoiceData { payments: vec![payment; media_types::MAX_INVOICE_PAYMENTS + 1], ..tax_invoice() },
        InvoiceData { taxes: vec![tax; media_types::MAX_INVOICE_TAXES + 1], ..tax_invoice() },
    ];
    for data in oversized {
        let job = InvoiceRenderJob { data, ..job() };
        assert_eq!(post_render(&server, push(&job), Some(&bearer())).await, 400);
        let key = invoice_pdf_key(job.tenant_id, job.invoice_id);
        assert_eq!(server.storage.head(Bucket::Private, &key).await.unwrap(), None, "nothing is stored");
    }
    assert!(server.bus.sent.lock().unwrap().is_empty());

    // At the limits, it renders, even with every description at its longest in four-byte characters: a body over
    // axum's default limit of 2 MiB.
    let longest = InvoiceLine { description: "\u{1D11E}".repeat(200), ..tax_invoice().lines[0].clone() };
    let at_limits = InvoiceData {
        lines: vec![longest; media_types::MAX_INVOICE_LINES],
        payments: vec![tax_invoice().payments[0].clone(); media_types::MAX_INVOICE_PAYMENTS],
        taxes: vec![tax_invoice().taxes[0].clone(); media_types::MAX_INVOICE_TAXES],
        ..tax_invoice()
    };
    let job = InvoiceRenderJob { data: at_limits, ..job() };
    let body = push(&job);
    assert!(body.len() > 2 * 1024 * 1024 && body.len() < render::MAX_RENDER_BODY, "{} bytes", body.len());
    assert_eq!(post_render(&server, body, Some(&bearer())).await, 200);
    server.storage.delete(Bucket::Private, &invoice_pdf_key(job.tenant_id, job.invoice_id)).await.unwrap();
}

/// What every report template prints at its head, with no day of the period left to close.
fn head(title: &str, period: &str) -> Value {
    json!({ "title": title, "subtitle": null, "property": "Galle Fort Hotels (Pvt) Ltd", "period": period,
            "not_closed": null })
}

fn with(mut head: Value, body: Value) -> Value {
    head.as_object_mut().unwrap().extend(body.as_object().unwrap().clone());
    head
}

/// The data of each template, as the reports module builds it: every figure already formatted.
fn report_fixture(template: ReportTemplate) -> Value {
    match template {
        ReportTemplate::Summary => with(
            json!({ "title": "Summary report", "subtitle": null, "property": "Galle Fort Hotels (Pvt) Ltd",
                    "period": "19 Oct 2026 – 25 Oct 2026", "not_closed": "Not closed yet: 25 Oct 2026" }),
            json!({
                "headline": [
                    { "label": "Occupancy", "value": "72.5%", "last_year": "70.0%" },
                    { "label": "ADR", "value": "Rs 12,345", "last_year": null },
                ],
                "days": [{ "date": "19 Oct 2026", "rooms": "29 / 40", "occupancy": "72.5%", "adr": "Rs 12,345",
                           "revpar": "Rs 8,950", "room_revenue": "Rs 358,005", "total_revenue": "Rs 401,220" }],
                "breakdowns": [{ "title": "By room type", "rows": [
                    { "key": "DLX", "room_nights": "29", "room_revenue": "Rs 358,005", "revenue": "Rs 401,220" },
                ] }],
            }),
        ),
        ReportTemplate::DailyList => with(
            head("Arrivals", "26 Oct 2026"),
            json!({
                "rows": [{ "room": "101", "guest": "Ada Perera", "confirmation": "GAL-000042",
                           "arrival": "26 Oct 2026", "departure": "28 Oct 2026", "status": "Confirmed",
                           "guests": "2 + 1" }],
                "count": "1 stay",
            }),
        ),
        ReportTemplate::TdlFormA => with(
            json!({ "title": "Form A", "subtitle": "Tourism Development Levy: tourist hotels",
                    "property": "Galle Fort Hotels (Pvt) Ltd", "period": "July – September (2026)",
                    "not_closed": null }),
            json!({
                "registration_no": "TDL/0042",
                "sales": [
                    { "label": "Room Sales", "value": "55,800" },
                    { "label": "Food Sales", "value": "237,973" },
                    { "label": "Bar Sales", "value": "3,183,537" },
                    { "label": "Other Sales", "value": "0" },
                ],
                "total_turnover": "3,477,310",
                "deductions": [
                    { "label": "Service Charge", "value": "23,797" },
                    { "label": "VAT", "value": "0" },
                ],
                "base": "3,453,513",
                "rate": "1%",
                "tdl": "34,535",
                "credit_note": null,
            }),
        ),
        ReportTemplate::Sscl => with(
            json!({ "title": "SSCL", "subtitle": "Social Security Contribution Levy",
                    "property": "Galle Fort Hotels (Pvt) Ltd", "period": "Q3 2026: July – September",
                    "not_closed": null }),
            json!({
                "months": ["July", "August", "September"],
                "rows": [
                    { "label": "Turnover", "values": ["Rs 100,000", "Rs 0", "Rs 0", "Rs 100,000"], "strong": false },
                    { "label": "Levy at 2.5%", "values": ["Rs 2,500", "Rs 0", "Rs 0", "Rs 2,500"], "strong": true },
                    { "label": "Payment due", "values": ["20 Aug 2026", "20 Sep 2026", "20 Oct 2026", ""],
                      "strong": false },
                ],
                "foreign_cash": { "label": "Foreign-currency cash (not exempt)",
                                  "values": ["Rs 0", "Rs 0", "Rs 0", "Rs 0"] },
                "return_due": "20 Oct 2026",
                "exemption_note": null,
                "bad_debts_note": "Bad debts: none until the system writes off debts.",
                "method_note": "Exempt: services paid in foreign currency by card or bank transfer (SSCL Act No. 25 \
                                of 2022, First Schedule Part II item 10), measured when each bill is settled.",
            }),
        ),
        ReportTemplate::Vat => with(
            head("VAT return", "Q3 2026 (2630)"),
            json!({
                "tin": "114567890",
                "period_code": "2630",
                "figures": [
                    { "label": "Value of supply", "value": "Rs 100,000" },
                    { "label": "Output VAT", "value": "Rs 18,000" },
                ],
                "schedule01": [{ "serial_no": "1", "date": "09/30/2026", "invoice_no": "INCL01_2630",
                                 "tin": "100902885", "name": "Purchasers without a TIN",
                                 "description": "Inclusive Invoices at 18%", "value": "100000",
                                 "vat": "18000" }],
                "schedule04": [],
                "timing_difference": [],
            }),
        ),
        ReportTemplate::ServiceCharge => with(
            head("Service charge", "August 2026"),
            json!({
                "rows": [
                    { "label": "Rooms", "collected": "Rs 5,580", "absorbed": "Rs 0" },
                    { "label": "Outlet BAR", "collected": "Rs 23,797", "absorbed": "Rs 1,200" },
                ],
                "total": { "collected": "Rs 29,377", "absorbed": "Rs 1,200" },
            }),
        ),
    }
}

#[test]
fn every_report_template_renders_its_fixture() {
    let expected: [(ReportTemplate, &[&str]); 6] = [
        (ReportTemplate::Summary, &["Summary report", "Not closed yet: 25 Oct 2026", "72.5%", "BY ROOM TYPE", "DLX"]),
        (ReportTemplate::DailyList, &["Arrivals", "Ada Perera", "GAL-000042", "1 stay"]),
        (
            ReportTemplate::TdlFormA,
            &[
                "Form A",
                "Ceylon Tourist – Tourist Development Levy (TDL)",
                "Institutions falling within the category of Tourist Hotel",
                "TDL/0042",
                "Amount in (Rs)",
                "Room Sales",
                "Total Turnover",
                "Service Charge",
                "Total Turnover less deductions",
                "34,535",
                "GovPay",
                "Due before the end of the following quarter",
                "Paid through GovPay",
                "Signature of Director / Proprietor",
                "Date",
            ],
        ),
        (ReportTemplate::Sscl, &["SSCL", "Levy at 2.5%", "20 Oct 2026", "Exempt: services paid in foreign currency"]),
        (
            ReportTemplate::Vat,
            &["VAT return", "2630", "114567890", "INCL01_2630", "Inclusive Invoices at 18% 100000 18000"],
        ),
        (ReportTemplate::ServiceCharge, &["Service charge", "August 2026", "Outlet BAR", "29,377"]),
    ];
    for (template, strings) in expected {
        let data = report_fixture(template);
        let pdf = render::report_pdf(template, &data).unwrap();
        assert!(pdf.starts_with(b"%PDF-"), "{template:?} is a PDF");
        let document = render::report_layout(template, &data).unwrap();
        for (at, page) in document.pages().iter().enumerate() {
            let size = page.frame.size();
            let (width, height) = (size.x.to_mm(), size.y.to_mm());
            // A4 portrait, but for the VAT schedules' landscape pages after the first.
            let (width, height) =
                if template == ReportTemplate::Vat && at > 0 { (height, width) } else { (width, height) };
            assert!((width - 210.0).abs() < 0.5 && (height - 297.0).abs() < 0.5, "{template:?} page {at}");
        }
        let text = text_of(&document);
        for expected in ["Galle Fort Hotels (Pvt) Ltd"].iter().chain(strings) {
            assert!(text.contains(expected), "{template:?}: {expected:?} is not in {text:?}");
        }
        assert_eq!(render::report_pdf(template, &data).unwrap(), pdf, "{template:?}: the same data, the same bytes");
    }
}

#[test]
fn data_a_template_cannot_read_is_refused() {
    let error = render::report_pdf(ReportTemplate::Summary, &json!({ "title": "Summary report" })).unwrap_err();
    assert!(error.to_string().starts_with("laying out the report failed"), "{error}");
}

fn report_job(template: ReportTemplate, data: Value) -> ReportRenderJob {
    ReportRenderJob { tenant_id: Uuid::now_v7(), report_id: Uuid::now_v7(), template, data }
}

async fn post_report(server: &Server, job: &ReportRenderJob, auth: Option<&str>) -> reqwest::StatusCode {
    let body = serde_json::to_vec(&envelope_on("report-render", &serde_json::to_value(job).unwrap())).unwrap();
    let mut request = http()
        .post(format!("http://{}/render-report", server.addr))
        .header("content-type", "application/json")
        .body(body);
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    request.send().await.unwrap().status()
}

#[tokio::test]
async fn a_report_push_stores_its_pdf_and_publishes_its_key() {
    let server = start().await;
    let job = report_job(ReportTemplate::TdlFormA, report_fixture(ReportTemplate::TdlFormA));
    assert_eq!(post_report(&server, &job, None).await, 401);
    assert_eq!(post_report(&server, &job, Some(&bearer())).await, 200);
    assert_eq!(post_report(&server, &job, Some(&bearer())).await, 200, "a redelivery overwrites");

    let key = report_pdf_key(job.tenant_id, job.report_id);
    let expected = MediaResult::ReportRendered { tenant_id: job.tenant_id, report_id: job.report_id, key: key.clone() };
    let sent = server.bus.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "each delivery publishes");
    for message in &sent {
        assert_eq!(message.topic, "media-results");
        assert_eq!(serde_json::from_value::<MediaResult>(message.payload.clone()).unwrap(), expected);
    }
    let stored = server.storage.get(Bucket::Private, &key).await.unwrap();
    assert_eq!(stored, render::report_pdf(job.template, &job.data).unwrap());
    assert_eq!(server.storage.head(Bucket::Public, &key).await.unwrap(), None, "a report is never public");
    server.storage.delete(Bucket::Private, &key).await.unwrap();
}

/// A job that can never render publishes its failure, so the report is marked failed, and nothing is stored.
#[tokio::test]
async fn a_report_that_cannot_render_publishes_its_failure() {
    let server = start().await;
    let unreadable = report_job(ReportTemplate::Sscl, json!({ "title": "SSCL" }));
    let padding = "x".repeat(media_types::MAX_REPORT_BYTES);
    let oversized =
        report_job(ReportTemplate::Summary, with(report_fixture(ReportTemplate::Summary), json!({ "pad": padding })));
    for job in [&unreadable, &oversized] {
        assert_eq!(post_report(&server, job, Some(&bearer())).await, 200);
        let key = report_pdf_key(job.tenant_id, job.report_id);
        assert_eq!(server.storage.head(Bucket::Private, &key).await.unwrap(), None, "nothing is stored");
    }
    let sent = server.bus.sent.lock().unwrap().clone();
    let results: Vec<MediaResult> =
        sent.iter().map(|message| serde_json::from_value(message.payload.clone()).unwrap()).collect();
    match &results[..] {
        [
            MediaResult::ReportFailed { report_id: first, reason: unreadable_reason, .. },
            MediaResult::ReportFailed { report_id: second, reason: oversized_reason, .. },
        ] => {
            assert_eq!((*first, *second), (unreadable.report_id, oversized.report_id));
            assert!(unreadable_reason.starts_with("laying out the report failed"), "{unreadable_reason}");
            assert!(oversized_reason.contains("too large"), "{oversized_reason}");
        }
        other => panic!("two failures, not {other:?}"),
    }
}

#[tokio::test]
async fn a_malformed_report_push_is_refused_for_good() {
    let server = start().await;
    let not_a_job = json!({ "tenant_id": Uuid::now_v7(), "report_id": Uuid::now_v7(), "template": "invoice" });
    let body = serde_json::to_vec(&envelope_on("report-render", &not_a_job)).unwrap();
    let status = http()
        .post(format!("http://{}/render-report", server.addr))
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(body)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 400);
    assert!(server.bus.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_report_render_answers_503_until_the_result_is_published() {
    let server = start_with(CaptureBus { fail: true, ..CaptureBus::default() }).await;
    let job = report_job(ReportTemplate::Vat, report_fixture(ReportTemplate::Vat));
    assert_eq!(post_report(&server, &job, Some(&bearer())).await, 503);
    server.storage.delete(Bucket::Private, &report_pdf_key(job.tenant_id, job.report_id)).await.unwrap();
}
