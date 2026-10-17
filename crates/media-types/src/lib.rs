//! The media job and result types shared by core-api, media-svc and the imaging worker, and the invoice data
//! media-svc renders. This crate holds no codecs, so core-api can depend on it without linking any.

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Thumb,
    Sm,
    Md,
    Lg,
    Xl,
}

impl Size {
    /// The long edge in pixels. The thumb is a square of this side.
    pub fn long_edge(self) -> u32 {
        match self {
            Self::Thumb => 320,
            Self::Sm => 640,
            Self::Md => 1024,
            Self::Lg => 1600,
            Self::Xl => 2400,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Sm => "sm",
            Self::Md => "md",
            Self::Lg => "lg",
            Self::Xl => "xl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Avif,
    Webp,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Avif => "avif",
            Self::Webp => "webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Gallery,
    IssuePhoto,
    GuestDocument,
}

impl Profile {
    pub fn sizes(self) -> &'static [Size] {
        match self {
            Self::Gallery => &[Size::Thumb, Size::Sm, Size::Md, Size::Lg, Size::Xl],
            Self::IssuePhoto => &[Size::Thumb, Size::Sm, Size::Md, Size::Lg],
            Self::GuestDocument => &[Size::Thumb, Size::Md],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessJob {
    pub tenant_id: Uuid,
    pub asset_id: Uuid,
    pub purpose: Profile,
    pub mime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenditionOut {
    pub size: Size,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaResult {
    Ready {
        tenant_id: Uuid,
        asset_id: Uuid,
        width: u32,
        height: u32,
        archive_key: String,
        renditions: Vec<RenditionOut>,
    },
    Failed {
        tenant_id: Uuid,
        asset_id: Uuid,
        reason: String,
    },
    /// An invoice's or credit note's PDF is stored at `key`.
    InvoiceRendered {
        tenant_id: Uuid,
        invoice_id: Uuid,
        key: String,
    },
}

/// An `invoice-render` job: lay `data` out as the PDF of `invoice_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceRenderJob {
    pub tenant_id: Uuid,
    pub invoice_id: Uuid,
    pub data: InvoiceData,
}

/// Whether a document is an invoice or a credit note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceKind {
    Invoice,
    CreditNote,
}

impl InvoiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invoice => "invoice",
            Self::CreditNote => "credit_note",
        }
    }
}

impl TryFrom<String> for InvoiceKind {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "invoice" => Ok(Self::Invoice),
            "credit_note" => Ok(Self::CreditNote),
            _ => Err(format!("unknown InvoiceKind {value:?}")),
        }
    }
}

/// Everything printed on an invoice or credit note, as it was when the document was issued. Every `_lkr` figure is
/// whole rupees; `total` and `amount` are minor units of their `currency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceData {
    pub kind: InvoiceKind,
    pub supplier: Supplier,
    pub purchaser: Purchaser,
    pub serial: String,
    /// "TAX INVOICE", "INVOICE" or "CREDIT NOTE".
    pub title: String,
    pub issued_on: Date,
    pub supply_from: Date,
    pub supply_to: Date,
    pub lines: Vec<InvoiceLine>,
    /// The on-bill taxes, one per tax and rate.
    pub taxes: Vec<InvoiceTax>,
    /// `total_lkr` less the taxes, so the printed figures always add up.
    pub net_lkr: i64,
    /// The sum of the lines' `total_lkr`.
    pub total_lkr: i64,
    /// `total_lkr` in English words.
    pub total_words: String,
    pub payments: Vec<InvoicePayment>,
    pub vat_registered: bool,
    /// The serial of the invoice a credit note credits.
    #[serde(default)]
    pub credits_serial: Option<String>,
    /// Why a credit note was issued.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supplier {
    pub legal_name: String,
    pub address: Option<String>,
    pub tin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purchaser {
    pub name: String,
    pub tin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub description: String,
    pub quantity: i32,
    pub currency: String,
    /// In minor units of `currency`, shown for reference on a foreign line.
    pub total: i64,
    pub total_lkr: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceTax {
    pub name: String,
    pub rate_bp: i32,
    pub amount_lkr: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoicePayment {
    pub method: String,
    /// In minor units of `currency`; a refund is negative.
    pub amount: i64,
    pub currency: String,
    /// `amount` in whole rupees, rounded half up, for a payment in LKR; `None` for one in another currency.
    #[serde(default)]
    pub amount_lkr: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_result_round_trips_with_its_wire_shape() {
        let tenant_id = Uuid::nil();
        let asset_id = Uuid::max();
        let result = MediaResult::Ready {
            tenant_id,
            asset_id,
            width: 4000,
            height: 3000,
            archive_key: "t/a/original.jxl".into(),
            renditions: vec![RenditionOut {
                size: Size::Thumb,
                format: Format::Avif,
                width: 320,
                height: 320,
                bytes: 1234,
                key: "t/a/thumb.avif".into(),
            }],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["renditions"][0]["size"], "thumb");
        assert_eq!(json["renditions"][0]["format"], "avif");
        assert_eq!(serde_json::from_value::<MediaResult>(json).unwrap(), result);
    }

    #[test]
    fn a_failed_result_and_a_job_round_trip() {
        let failed = MediaResult::Failed { tenant_id: Uuid::nil(), asset_id: Uuid::nil(), reason: "corrupt".into() };
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["type"], "failed");
        assert_eq!(serde_json::from_value::<MediaResult>(json).unwrap(), failed);

        let job = ProcessJob {
            tenant_id: Uuid::nil(),
            asset_id: Uuid::nil(),
            purpose: Profile::IssuePhoto,
            mime: "image/jpeg".into(),
        };
        let json = serde_json::to_value(&job).unwrap();
        assert_eq!(json["purpose"], "issue_photo");
        assert_eq!(serde_json::from_value::<ProcessJob>(json).unwrap(), job);
    }

    #[test]
    fn sizes_are_named_and_ordered_by_long_edge() {
        let all = Profile::Gallery.sizes();
        assert_eq!(all.iter().map(|s| s.as_str()).collect::<Vec<_>>(), ["thumb", "sm", "md", "lg", "xl"]);
        assert_eq!(all.iter().map(|s| s.long_edge()).collect::<Vec<_>>(), [320, 640, 1024, 1600, 2400]);
        assert_eq!(Profile::GuestDocument.sizes(), [Size::Thumb, Size::Md]);
        assert_eq!(Profile::IssuePhoto.sizes().len(), 4);
        assert_eq!((Format::Avif.as_str(), Format::Webp.as_str()), ("avif", "webp"));
    }

    fn credit_note() -> InvoiceData {
        InvoiceData {
            kind: InvoiceKind::CreditNote,
            supplier: Supplier { legal_name: "Galle Ltd".into(), address: None, tin: Some("123456789".into()) },
            purchaser: Purchaser { name: "Acme".into(), tin: None },
            serial: "CN-GAL-2026-000001".into(),
            title: "CREDIT NOTE".into(),
            issued_on: Date::from_calendar_date(2026, time::Month::October, 17).unwrap(),
            supply_from: Date::from_calendar_date(2026, time::Month::October, 15).unwrap(),
            supply_to: Date::from_calendar_date(2026, time::Month::October, 16).unwrap(),
            lines: vec![InvoiceLine {
                description: "Room".into(),
                quantity: -1,
                currency: "USD".into(),
                total: -10_000,
                total_lkr: -30_000,
            }],
            taxes: vec![InvoiceTax { name: "Tourism development levy".into(), rate_bp: 100, amount_lkr: -297 }],
            net_lkr: -29_703,
            total_lkr: -30_000,
            total_words: "Minus Thirty Thousand Rupees Only".into(),
            payments: vec![InvoicePayment {
                method: "cash".into(),
                amount: -10_000,
                currency: "USD".into(),
                amount_lkr: None,
            }],
            vat_registered: false,
            credits_serial: Some("GAL-2026-000001".into()),
            reason: Some("Billed twice".into()),
        }
    }

    #[test]
    fn invoice_data_round_trips_with_its_wire_shape() {
        let data = credit_note();
        let json = serde_json::to_value(&data).unwrap();
        assert_eq!(json["kind"], "credit_note");
        assert_eq!(json["issued_on"], "2026-10-17");
        assert_eq!(serde_json::from_value::<InvoiceData>(json).unwrap(), data);
        assert_eq!(InvoiceKind::try_from("invoice".to_owned()), Ok(InvoiceKind::Invoice));
        assert!(InvoiceKind::try_from("receipt".to_owned()).is_err());
    }

    #[test]
    fn a_render_job_and_its_result_round_trip_with_their_wire_shape() {
        let (tenant_id, invoice_id) = (Uuid::nil(), Uuid::max());
        // The shape folio queues on `invoice-render`.
        let queued = serde_json::json!({ "tenant_id": tenant_id, "invoice_id": invoice_id, "data": credit_note() });
        let job: InvoiceRenderJob = serde_json::from_value(queued).unwrap();
        assert_eq!(job, InvoiceRenderJob { tenant_id, invoice_id, data: credit_note() });

        let rendered = MediaResult::InvoiceRendered { tenant_id, invoice_id, key: "t/a/invoices/b.pdf".into() };
        let json = serde_json::to_value(&rendered).unwrap();
        assert_eq!(json["type"], "invoice_rendered");
        assert_eq!(json["key"], "t/a/invoices/b.pdf");
        assert_eq!(serde_json::from_value::<MediaResult>(json).unwrap(), rendered);
    }
}
