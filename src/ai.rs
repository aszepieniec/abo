//! One POST to /v1/messages. No tools, no agent loop: the model reads a
//! document and returns one JSON proposal. It never decides anything — see
//! DESIGN.md §8, "The model is an untrusted input source".

use crate::repo::{Ctx, Vat};
use anyhow::{bail, Context as _, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const API: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-opus-5";
const EFFORT: &str = "medium";
const MAX_TOKENS: u32 = 16000;

#[derive(Serialize, Deserialize, Clone)]
pub struct Proposal {
    pub date: String,
    pub vendor: String,
    pub description: String,
    pub currency: String,
    pub postings: Vec<ProposedPosting>,
    pub invoice_ref: String,
    pub confidence: String,
    /// Non-empty means the model is asking rather than guessing. Blocks
    /// auto-approval even under --yes.
    pub questions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ProposedPosting {
    pub account: String,
    /// A decimal string. Never a JSON number: that is an f64 by the time serde
    /// sees it, and money does not survive the trip.
    pub amount: String,
}

/// The conversation about one document, keyed by its sha256. The document
/// itself is not stored here — it is re-read from .abo/pending on every
/// request, so the thread stays small and there is one copy of the bytes.
#[derive(Serialize, Deserialize, Default)]
pub struct Thread {
    pub ext: String,
    pub media_type: String,
    pub turns: Vec<Turn>,
    pub proposal: Option<Proposal>,
}

#[derive(Serialize, Deserialize)]
pub struct Turn {
    pub role: String,
    pub text: String,
}

pub struct Document {
    pub media_type: String,
    pub bytes: Vec<u8>,
}

impl Document {
    /// Content block for the document. PDFs are `document`, everything else
    /// `image`; the media type comes from sniffing the bytes, never from a
    /// client-supplied filename.
    fn block(&self) -> Value {
        let kind = if self.media_type == "application/pdf" { "document" } else { "image" };
        json!({
            "type": kind,
            "source": {
                "type": "base64",
                "media_type": self.media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(&self.bytes),
            }
        })
    }
}

pub async fn propose(ctx: &Ctx, doc: &Document, thread: &Thread) -> Result<Proposal> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY is not set")?;

    // The document goes before the text instruction.
    let mut messages: Vec<Value> = Vec::new();
    for (i, t) in thread.turns.iter().enumerate() {
        if i == 0 {
            messages.push(json!({
                "role": "user",
                "content": [doc.block(), {"type": "text", "text": t.text}]
            }));
        } else {
            messages.push(json!({"role": t.role, "content": t.text}));
        }
    }

    let body = json!({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        // Thinking is on by default on this model; effort is the dial.
        "output_config": { "effort": EFFORT, "format": { "type": "json_schema", "schema": schema() } },
        "system": [{
            "type": "text",
            "text": system_prompt(ctx)?,
            // Stable across every bill for this company, so it caches.
            "cache_control": { "type": "ephemeral" }
        }],
        "messages": messages,
    });

    let res = reqwest::Client::new()
        .post(API)
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("calling the Anthropic API")?;

    let status = res.status();
    let v: Value = res.json().await.context("reading the API response")?;
    if !status.is_success() {
        bail!("api {status}: {}", v["error"]["message"].as_str().unwrap_or(&v.to_string()));
    }
    match v["stop_reason"].as_str() {
        Some("refusal") => bail!(
            "the model declined to read this document ({})",
            v["stop_details"]["category"].as_str().unwrap_or("no category")
        ),
        Some("max_tokens") => bail!("the response was cut off; the document may be too long"),
        _ => {}
    }

    let text = v["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .context("no text block in the response")?;
    serde_json::from_str(text).with_context(|| format!("proposal was not the agreed shape: {text}"))
}

/// The agreed shape. `additionalProperties: false` and a complete `required`
/// are both mandatory for structured outputs.
fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["date", "vendor", "description", "currency", "postings",
                     "invoice_ref", "confidence", "questions"],
        "properties": {
            "date": {"type": "string", "format": "date",
                     "description": "invoice date, ISO 8601"},
            "vendor": {"type": "string", "description": "supplier name as printed"},
            "description": {"type": "string", "description": "what was bought, one short line"},
            "currency": {"type": "string", "description": "ISO 4217 code, e.g. CHF"},
            "postings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["account", "amount"],
                    "properties": {
                        "account": {"type": "string", "description": "an already declared account"},
                        "amount": {"type": "string",
                                   "description": "decimal string, e.g. 100.00; negative for credits; must sum to zero"}
                    }
                }
            },
            "invoice_ref": {"type": "string", "description": "the supplier's own invoice number, or \"\""},
            "confidence": {"type": "string", "enum": ["high", "medium", "low"]},
            "questions": {"type": "array", "items": {"type": "string"},
                          "description": "ask instead of guessing; any question blocks approval"}
        }
    })
}

fn system_prompt(ctx: &Ctx) -> Result<String> {
    let c = &ctx.cfg.company;
    let accounts = crate::journal::declared_accounts(ctx)?.join("\n");
    let vat = match &ctx.cfg.vat {
        Vat::None => "\
The company is not VAT registered. Never post to a VAT account. Book gross amounts."
            .to_string(),
        Vat::Saldo { rate, account } => format!("\
The company uses the Swiss net tax rate method (Saldosteuersatz), industry rate {rate}%.
No input VAT is deductible, ever. Book the gross amount as a single expense posting.
Never propose a posting to an input VAT account. {account} is used only by the VAT
return, never by a bill."),
        Vat::Effective { input, .. } => format!("\
The company uses the Swiss effective method. Deductible input VAT goes to {input}.
Swiss VAT rates are 8.1% standard, 2.6% reduced (food, non-alcoholic drinks, books,
medicines), 3.8% accommodation. Check that the stated VAT equals net x rate.

Switzerland is a third country for EU VAT, so there is no intra-community anything.
A foreign supplier's invoice showing no VAT carries NO deductible input tax: Swiss
import VAT is assessed by customs (BAZG) and arrives on a separate document. Book
the full invoice as expense and post no input VAT.

Services bought from abroad (hosting, SaaS, foreign consultants) are subject to
acquisition tax (Bezugsteuer), self-assessed. If this bill looks like one, say so
in questions rather than guessing the postings."),
    };

    Ok(format!("\
You are booking a supplier bill for {name}{uid}, a company in Switzerland.
Its books are in hledger format and its currency is {currency}.

Read the attached document and propose one journal entry.

{vat}

Rules:
- Use ONLY accounts from this list. Never invent one. If nothing fits, leave the
  posting out and ask in questions.
- Postings must sum to exactly zero in each currency.
- An unpaid bill credits liabilities:payable. A bill already paid from the bank
  credits assets:bank. If you cannot tell from the document, ask.
- Amounts are decimal strings exactly as printed, e.g. \"1234.55\". Credits are
  negative.
- If the document is not a bill, or you cannot read the total, say so in questions
  rather than inventing numbers.

Declared accounts:
{accounts}",
        name = c.name,
        uid = if c.uid.is_empty() { String::new() } else { format!(" (VAT no. {})", c.uid) },
        currency = c.currency,
    ))
}
