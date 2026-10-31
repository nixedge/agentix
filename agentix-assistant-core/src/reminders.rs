use crate::{bounded_text, Error, InferenceContext, Result, TellRequest, TellResponse, Turn};
use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_REMINDERS: usize = 1000;
pub fn reminder_local_reference(instant: DateTime<Utc>, zone: &str) -> Result<String> {
    let tz: chrono_tz::Tz = zone
        .parse()
        .map_err(|_| Error::Invalid("invalid reminder timezone".into()))?;
    Ok(instant.with_timezone(&tz).to_rfc3339())
}

pub fn reminder_spoken_time(
    due: DateTime<Utc>,
    reference: DateTime<Utc>,
    zone: &str,
) -> Result<String> {
    let tz: chrono_tz::Tz = zone
        .parse()
        .map_err(|_| Error::Invalid("invalid reminder timezone".into()))?;
    let local = due.with_timezone(&tz);
    let today = reference.with_timezone(&tz).date_naive();
    let day = if local.date_naive() == today {
        "today".to_string()
    } else if Some(local.date_naive()) == today.succ_opt() {
        "tomorrow".to_string()
    } else {
        local.format("%A, %B %-d").to_string()
    };
    let time = local.format("%-I:%M %p").to_string().replace(":00", "");
    Ok(format!("{day} at {time}"))
}

pub fn reminder_uuid(id: Uuid) -> Result<()> {
    if id.is_nil() {
        return Err(Error::Invalid("nil reminder/device identity".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTurnRequest {
    pub device_id: Uuid,
    pub request: TellRequest,
}
impl DeviceTurnRequest {
    pub fn validate(&self) -> Result<()> {
        reminder_uuid(self.device_id)?;
        self.request.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub id: Uuid,
    pub revision: u64,
    pub text: String,
    pub due_at: DateTime<Utc>,
    pub timezone: String,
    pub cancelled: bool,
}
impl Reminder {
    pub fn local_due(&self) -> Result<String> {
        let tz: chrono_tz::Tz = self
            .timezone
            .parse()
            .map_err(|_| Error::Invalid("invalid reminder timezone".into()))?;
        Ok(self.due_at.with_timezone(&tz).to_rfc3339())
    }
    pub fn validate(&self) -> Result<()> {
        reminder_uuid(self.id)?;
        bounded_text(&self.text, 1024)?;
        if self.revision == 0 || self.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(Error::Invalid("invalid reminder revision/timezone".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTurnResponse {
    pub device_id: Uuid,
    pub response: TellResponse,
    pub reminder_receipts: Vec<Reminder>,
}
impl DeviceTurnResponse {
    pub fn validate(&self, request: &DeviceTurnRequest) -> Result<()> {
        if self.device_id != request.device_id || self.reminder_receipts.len() > 1 {
            return Err(Error::Invalid("invalid reminder response identity".into()));
        }
        self.response.validate(&request.request)?;
        for receipt in &self.reminder_receipts {
            receipt.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderDeliveryState {
    Armed,
    Blocked,
    PostPending,
    Posted,
    Ringing,
    Dismissed,
    Uncertain,
    CancelApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderReport {
    pub report_id: Uuid,
    pub reminder_id: Uuid,
    pub revision: u64,
    pub sequence: u64,
    pub state: ReminderDeliveryState,
}
impl ReminderReport {
    pub fn validate(&self) -> Result<()> {
        reminder_uuid(self.report_id)?;
        reminder_uuid(self.reminder_id)?;
        if self.revision == 0 || self.sequence == 0 {
            return Err(Error::Invalid("invalid delivery sequence".into()));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderSnapshot {
    pub server_id: Uuid,
    pub device_id: Uuid,
    pub sequence: u64,
    pub reminders: Vec<Reminder>,
    pub observations: Vec<ReminderReport>,
}
impl ReminderSnapshot {
    pub fn validate(&self) -> Result<()> {
        reminder_uuid(self.server_id)?;
        reminder_uuid(self.device_id)?;
        if self.reminders.len() > MAX_REMINDERS || self.observations.len() > MAX_REMINDERS {
            return Err(Error::Invalid("invalid reminder snapshot bounds".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for item in &self.reminders {
            item.validate()?;
            if !ids.insert(item.id) {
                return Err(Error::Invalid("duplicate reminder".into()));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for item in &self.observations {
            item.validate()?;
            if self.reminders.iter().any(|r| {
                r.id == item.reminder_id
                    && ((item.state == ReminderDeliveryState::CancelApplied) != r.cancelled)
            }) {
                return Err(Error::Invalid(
                    "observation disagrees with desired cancellation".into(),
                ));
            }
            if !seen.insert(item.reminder_id)
                || !self
                    .reminders
                    .iter()
                    .any(|r| r.id == item.reminder_id && r.revision == item.revision)
            {
                return Err(Error::Invalid("orphan/duplicate device observation".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderReports {
    pub server_id: Uuid,
    pub device_id: Uuid,
    pub reports: Vec<ReminderReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReminderTime {
    After { seconds: u64 },
    Local { date: NaiveDate, time: NaiveTime },
}
impl ReminderTime {
    pub fn resolve(&self, accepted: DateTime<Utc>, zone: &str) -> Result<DateTime<Utc>> {
        let tz: chrono_tz::Tz = zone
            .parse()
            .map_err(|_| Error::Invalid("invalid reminder timezone".into()))?;
        let instant = match self {
            Self::After { seconds } if (1..=315_360_000).contains(seconds) => {
                accepted.checked_add_signed(Duration::seconds(*seconds as i64))
            }
            Self::Local { date, time } => tz
                .from_local_datetime(&date.and_time(*time))
                .single()
                .map(|at| at.with_timezone(&Utc)),
            _ => None,
        }
        .ok_or_else(|| {
            Error::Invalid(
                "Please clarify the reminder date and time, including ambiguous clock changes."
                    .into(),
            )
        })?;
        if instant <= accepted {
            return Err(Error::Invalid(
                "Please specify a future reminder time.".into(),
            ));
        }
        if instant.signed_duration_since(accepted) > Duration::seconds(315_360_000) {
            return Err(Error::Invalid(
                "Please choose a reminder within the next ten years.".into(),
            ));
        }
        Ok(instant)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReminderIntent {
    Other,
    Create {
        text: String,
        time: ReminderTime,
    },
    List,
    Reschedule {
        id: Uuid,
        expected_revision: u64,
        time: ReminderTime,
    },
    Cancel {
        id: Uuid,
        expected_revision: u64,
    },
    Clarify {
        question: String,
    },
}

#[derive(Debug, Clone)]
pub struct ReminderContext {
    pub context: InferenceContext,
    pub reminders: Vec<Reminder>,
    pub incomplete: bool,
}

#[async_trait]
pub trait ReminderInference: Send + Sync {
    async fn propose_reminder(&self, context: ReminderContext) -> Result<ReminderIntent>;
}

#[async_trait]
pub trait Reminders: Send + Sync {
    async fn enroll(&self, device_id: Uuid) -> Result<ReminderSnapshot>;
    async fn snapshot(&self, device_id: Uuid) -> Result<ReminderSnapshot>;
    async fn report(&self, reports: ReminderReports) -> Result<Vec<Uuid>>;
    async fn device_bound(&self, request_id: Uuid) -> Result<bool>;
    async fn begin_device_turn(
        &self,
        request: DeviceTurnRequest,
    ) -> Result<(Turn, Option<DeviceTurnResponse>)>;
    async fn finish_device_turn(
        &self,
        request: DeviceTurnRequest,
        intent: ReminderIntent,
        ordinary: Option<TellResponse>,
    ) -> Result<DeviceTurnResponse>;
}
