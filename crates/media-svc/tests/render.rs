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
    Supplier,
};
use storage::{Bucket, invoice_pdf_key};
use time::macros::date;
use typst::layout::{Frame, FrameItem};
use uuid::Uuid;

/// A VAT-registered hotel's tax invoice: a room charged in USD and a restaurant bill in LKR.
fn tax_invoice() -> InvoiceData {
    InvoiceData {
        kind: InvoiceKind::Invoice,
        supplier: Supplier {
            legal_name: "Galle Fort Hotels (Pvt) Ltd".into(),
            address: Some("28 Church Street\nGalle Fort, Galle 80000".into()),
            tin: Some("114567890".into()),
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
            InvoiceTax { name: "Service charge".into(), rate_bp: 1000, amount_lkr: 1_000 },
            InvoiceTax { name: "Tourism development levy".into(), rate_bp: 100, amount_lkr: 700 },
            InvoiceTax { name: "SSCL".into(), rate_bp: 250, amount_lkr: 1_943 },
            InvoiceTax { name: "VAT".into(), rate_bp: 1800, amount_lkr: 14_341 },
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
        taxes: vec![InvoiceTax { name: "VAT".into(), rate_bp: 1800, amount_lkr: -2_048 }],
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
    fn walk(frame: &Frame, out: &mut Vec<String>) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => walk(&group.frame, out),
                FrameItem::Text(text) => out.push(text.text.to_string()),
                _ => {}
            }
        }
    }
    let document = render::layout(data).unwrap();
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
    let id = Uuid::now_v7().to_string();
    PushEnvelope {
        message: PushMessage {
            data: STANDARD.encode(serde_json::to_vec(payload).unwrap()),
            message_id: id.clone(),
            attributes: BTreeMap::from([("message_id".to_owned(), id)]),
        },
        subscription: "push/invoice-render".to_owned(),
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
