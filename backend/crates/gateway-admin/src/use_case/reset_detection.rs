use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::runtime::SnapshotControl;

use crate::{
    model::{
        AdminError, MutationContext,
        reset_detection::{
            MIN_POLL_INTERVAL_SECS, ReplaceResetDetectionSettings, ResetDetectionSettings,
        },
    },
    ports::store::ResetDetectionStore,
};

use super::{map_store_error, publish_committed};

#[async_trait]
pub trait ResetDetectionService: Send + Sync {
    async fn load(&self) -> Result<ResetDetectionSettings, AdminError>;
    async fn replace(
        &self,
        context: &MutationContext,
        command: ReplaceResetDetectionSettings,
    ) -> Result<ResetDetectionSettings, AdminError>;
}

pub(crate) struct DefaultResetDetectionService {
    store: Arc<dyn ResetDetectionStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultResetDetectionService {
    #[must_use]
    pub(crate) fn new(
        store: Arc<dyn ResetDetectionStore>,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self { store, snapshot }
    }
}

#[async_trait]
impl ResetDetectionService for DefaultResetDetectionService {
    async fn load(&self) -> Result<ResetDetectionSettings, AdminError> {
        self.store
            .load_reset_detection_settings()
            .await
            .map_err(|error| map_store_error(error, "reset detection settings"))
    }

    async fn replace(
        &self,
        context: &MutationContext,
        command: ReplaceResetDetectionSettings,
    ) -> Result<ResetDetectionSettings, AdminError> {
        validate_reset_detection_settings(&command)?;
        let mutation = self
            .store
            .replace_reset_detection_settings(command, context)
            .await
            .map_err(|error| map_store_error(error, "reset detection settings"))?;
        publish_committed(self.snapshot.as_ref(), mutation.config_revision).await?;
        Ok(mutation.settings)
    }
}

fn validate_reset_detection_settings(
    command: &ReplaceResetDetectionSettings,
) -> Result<(), AdminError> {
    if command.poll_interval_secs < MIN_POLL_INTERVAL_SECS {
        Err(AdminError::invalid("轮询间隔不能低于 30 秒"))
    } else {
        Ok(())
    }
}
