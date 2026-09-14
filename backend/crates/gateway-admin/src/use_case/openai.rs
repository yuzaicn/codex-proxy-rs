//! OpenAI 管理资源的中立 ProviderAdmin 委托。

use std::sync::Arc;

use async_trait::async_trait;
use gateway_core::runtime::SnapshotControl;

use crate::{
    model::{
        AdminError,
        provider_credentials::{
            AuthorizationStarted, CompleteAuthorization, CredentialDeletion,
            CredentialDeletionResult, CredentialImportCommit, CredentialImportResult,
            CredentialMutationResult, ImportCredentials, PrepareCredentialImport,
            PrepareCredentialRotation, RotateCredential, StartAuthorization,
        },
    },
    ports::{provider::ProviderAdmin, store::AccountStore},
};

use super::{
    commit_authorization, commit_credential_rotation, delete_credentials, map_provider_error,
    map_store_error, pending_authorization, publish_committed,
    publish_credentials_and_observe_quota, required_credential, validate_authorization_commit,
    validate_prepared_import, validate_prepared_rotation,
};

/// OpenAI 固定管理路由消费的服务。
#[async_trait]
pub trait OpenAiService: Send + Sync {
    async fn import_document(
        &self,
        command: ImportCredentials,
    ) -> Result<CredentialImportResult, AdminError>;
    async fn start_authorization(
        &self,
        command: StartAuthorization,
    ) -> Result<AuthorizationStarted, AdminError>;
    async fn complete_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<CredentialMutationResult, AdminError>;
    async fn rotate(
        &self,
        command: RotateCredential,
    ) -> Result<CredentialMutationResult, AdminError>;
    async fn delete(
        &self,
        command: CredentialDeletion,
    ) -> Result<CredentialDeletionResult, AdminError>;
}

pub(crate) struct DefaultOpenAiService {
    provider: Arc<dyn ProviderAdmin>,
    accounts: Arc<dyn AccountStore>,
    proxies: Arc<dyn crate::ports::proxy::ProxyStore>,
    snapshot: Arc<dyn SnapshotControl>,
}

impl DefaultOpenAiService {
    #[must_use]
    pub(crate) fn new(
        provider: Arc<dyn ProviderAdmin>,
        accounts: Arc<dyn AccountStore>,
        proxies: Arc<dyn crate::ports::proxy::ProxyStore>,
        snapshot: Arc<dyn SnapshotControl>,
    ) -> Self {
        Self {
            provider,
            accounts,
            proxies,
            snapshot,
        }
    }
}

#[async_trait]
impl OpenAiService for DefaultOpenAiService {
    async fn import_document(
        &self,
        command: ImportCredentials,
    ) -> Result<CredentialImportResult, AdminError> {
        let context = command.context;
        let proxy_reservation = super::import_proxy_binding(
            self.proxies.as_ref(),
            command.outbound_proxy_id.as_deref(),
        )
        .await?;
        let outbound_proxy = proxy_reservation
            .as_ref()
            .map(|reservation| reservation.binding.clone());
        let prepared = self
            .provider
            .prepare_import(PrepareCredentialImport {
                default_outbound_proxy: outbound_proxy
                    .as_ref()
                    .map(|binding| binding.proxy.clone()),
                document: command.document,
            })
            .await
            .map_err(|error| map_provider_error(error, "OpenAI credential import"))?;
        validate_prepared_import(
            self.provider.provider_kind(),
            &prepared,
            "OpenAI credential import",
        )?;
        if prepared.credentials.is_empty() && !prepared.failures.is_empty() {
            let failures = prepared.failures;
            drop(proxy_reservation);
            return Ok(CredentialImportResult {
                // No credentials were committed, so no configuration revision was produced.
                config_revision: None,
                credential_ids: Vec::new(),
                created_count: 0,
                updated_count: 0,
                failures,
            });
        }
        let result = self
            .accounts
            .commit_credential_import(
                CredentialImportCommit {
                    outbound_proxy,
                    prepared,
                    settings: command.settings,
                },
                &context,
            )
            .await
            .map_err(|error| map_store_error(error, "OpenAI credential import"))?;
        drop(proxy_reservation);
        // 全失败时不会发生 store 提交，也就没有配置版本可发布；此时上面的分支已提前返回。
        let committed_revision = result
            .config_revision
            .ok_or_else(|| AdminError::internal("导入提交未返回配置版本"))?;
        publish_credentials_and_observe_quota(
            &self.provider,
            self.snapshot.as_ref(),
            committed_revision,
            &result.credential_ids,
            &context.request_id,
        )
        .await?;
        Ok(result)
    }

    async fn start_authorization(
        &self,
        command: StartAuthorization,
    ) -> Result<AuthorizationStarted, AdminError> {
        let pending = pending_authorization(
            self.accounts.as_ref(),
            self.proxies.as_ref(),
            self.provider.provider_kind(),
            &command,
            "OpenAI credential",
        )
        .await?;
        self.provider
            .start_authorization(pending)
            .await
            .map_err(|error| map_provider_error(error, "OpenAI authorization"))
    }

    async fn complete_authorization(
        &self,
        mut command: CompleteAuthorization,
    ) -> Result<CredentialMutationResult, AdminError> {
        let context = command.context.clone();
        let settings = command.settings.take();
        let prepared = self
            .provider
            .complete_authorization(command)
            .await
            .map_err(|error| map_provider_error(error, "OpenAI authorization"))?;
        let prepared = validate_authorization_commit(
            self.provider.provider_kind(),
            &context,
            prepared,
            "OpenAI authorization",
        )
        .await?;
        let result = commit_authorization(
            self.accounts.as_ref(),
            prepared,
            settings,
            &context,
            "OpenAI authorization",
        )
        .await?;
        publish_credentials_and_observe_quota(
            &self.provider,
            self.snapshot.as_ref(),
            result.config_revision,
            std::slice::from_ref(&result.account_id),
            &context.request_id,
        )
        .await?;
        Ok(result)
    }

    async fn rotate(
        &self,
        command: RotateCredential,
    ) -> Result<CredentialMutationResult, AdminError> {
        let context = command.mutation.context;
        let account_id = command.mutation.account_id;
        let details = required_credential(
            self.accounts.as_ref(),
            self.provider.provider_kind(),
            &account_id,
            "OpenAI credential rotation",
        )
        .await?;
        let account = details.credential;
        let prepared = self
            .provider
            .prepare_rotation(PrepareCredentialRotation {
                account: account.clone(),
                provider_material: command.provider_material,
            })
            .await
            .map_err(|error| map_provider_error(error, "OpenAI credential rotation"))?;
        validate_prepared_rotation(&account, &prepared, "OpenAI credential rotation")?;
        let result = commit_credential_rotation(
            self.accounts.as_ref(),
            prepared,
            &context,
            "OpenAI credential rotation",
        )
        .await?;
        self.provider
            .account_facts_changed(std::slice::from_ref(&result.account_id))
            .await;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(result)
    }

    async fn delete(
        &self,
        command: CredentialDeletion,
    ) -> Result<CredentialDeletionResult, AdminError> {
        let result = delete_credentials(
            self.accounts.as_ref(),
            self.provider.as_ref(),
            command,
            "OpenAI credential",
        )
        .await?;
        publish_committed(self.snapshot.as_ref(), result.config_revision).await?;
        Ok(result)
    }
}
