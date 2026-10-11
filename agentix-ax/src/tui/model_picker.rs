use agentix_api::models::{CodingSupport, ModelReadiness, ModelRecord, Residency};
use agentix_harness::client::GatewayClient;
use anyhow::{ensure, Result};
use tokio::sync::oneshot;

pub struct ModelPicker {
    pub items: Vec<ModelRecord>,
    pub selected: usize,
    pub filter: String,
    pub message: String,
    catalog: Option<oneshot::Receiver<Result<Vec<ModelRecord>>>>,
    selection: Option<oneshot::Receiver<Result<ModelReadiness>>>,
}

impl ModelPicker {
    pub fn open(gateway: String) -> Self {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = async { GatewayClient::new(&gateway)?.models().await }.await;
            let _ = tx.send(result);
        });
        Self {
            items: vec![],
            selected: 0,
            filter: String::new(),
            message: "Fetching installed models…".into(),
            catalog: Some(rx),
            selection: None,
        }
    }

    pub fn visible(&self) -> Vec<&ModelRecord> {
        let filter = self.filter.to_lowercase();
        self.items
            .iter()
            .filter(|m| m.id.as_str().to_lowercase().contains(&filter))
            .collect()
    }

    pub fn loading(&self) -> bool {
        self.catalog.is_some() || self.selection.is_some()
    }

    pub fn choose(&mut self, gateway: String, context: String) {
        if self.loading() {
            return;
        }
        let Some(model) = self.visible().get(self.selected).copied() else {
            return;
        };
        if let CodingSupport::Unsupported { reason } = &model.coding_support {
            self.message = reason.as_str().into();
            return;
        }
        if model.resident == Residency::Busy {
            self.message = "Model is busy; refresh when its current turn finishes".into();
            return;
        }
        let id = model.id.as_str().to_owned();
        self.message = format!("Loading and checking {id}… Esc cancels selection");
        let (tx, rx) = oneshot::channel();
        self.selection = Some(rx);
        tokio::spawn(async move {
            let result = prepare_selection(&gateway, &id, &context).await;
            let _ = tx.send(result);
        });
    }

    // Each picker owns its result channel. Closing/reopening drops the old receiver,
    // so an earlier warmup can never commit over a newer selection.
    pub fn poll(&mut self) -> Option<ModelReadiness> {
        if let Some(mut rx) = self.catalog.take() {
            match rx.try_recv() {
                Ok(Ok(items)) => {
                    self.message = if items.is_empty() {
                        "No installed models".into()
                    } else {
                        "↑↓ choose · Enter load/select · type to filter · Ctrl+R refresh · Esc close".into()
                    };
                    self.items = items;
                }
                Ok(Err(error)) => self.message = format!("Catalog failed: {error}"),
                Err(oneshot::error::TryRecvError::Empty) => self.catalog = Some(rx),
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.message = "Catalog request stopped".into()
                }
            }
        }
        if let Some(mut rx) = self.selection.take() {
            match rx.try_recv() {
                Ok(Ok(ready)) => return Some(ready),
                Ok(Err(error)) => {
                    self.message = format!("Selection failed: {error}. Current model unchanged.")
                }
                Err(oneshot::error::TryRecvError::Empty) => self.selection = Some(rx),
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.message = "Selection request stopped; current model unchanged".into()
                }
            }
        }
        None
    }
}

pub async fn prepare_selection(gateway: &str, id: &str, context: &str) -> Result<ModelReadiness> {
    let client = GatewayClient::new(gateway)?;
    let ready = client.readiness(id).await?;
    ensure!(
        ready.model_id.as_str() == id,
        "server returned a different model"
    );
    ensure!(
        ready.residency == Residency::Ready,
        "model is not ready ({:?})",
        ready.residency
    );
    ensure!(
        ready.coding_supported,
        "model has no supported coding template"
    );
    let tokens = client.tokenize(id, context).await?;
    // Reserve space for tool schemas, template framing, the next prompt and output.
    let required = tokens.count.saturating_add(4096);
    ensure!(required <= u64::from(tokens.context_limit.min(ready.context_limit)),
        "conversation needs at least {required} tokens including coding reserve; model context is {}", ready.context_limit);
    Ok(ready)
}
