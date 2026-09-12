use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use futures::future::BoxFuture;
use gateway_core::{
    engine::execution::{
        AuthenticatedClient, ClientAuthenticationError, ExecutionService, StartExecution,
        StartProviderExecution, StartedExecution,
    },
    error::GatewayError,
    health::{
        HealthProbe, HealthState, WorkerHealthKey, WorkerHealthSnapshot, WorkerHealthSource,
        WorkerRuntimeState,
    },
    routing::PublicModelId,
    task::{WorkerId, WorkerKind},
};
use tower::ServiceExt;

#[tokio::test]
async fn healthz_should_return_no_content_when_all_inputs_are_healthy() {
    let response = crate::openai::api_router(Arc::new(UnusedExecution))
        .await
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("health response");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn healthz_should_ignore_non_critical_worker_failures() {
    let worker_health = StaticWorkerHealth(
        [
            WorkerKind::OAuthRefresh,
            WorkerKind::QuotaCatalogHealth,
            WorkerKind::IntelligenceDetection,
            WorkerKind::ResetDetection,
        ]
        .into_iter()
        .map(|kind| worker_snapshot(kind, WorkerRuntimeState::BackingOff))
        .collect(),
    );
    let response = crate::openai::api_router_with_worker_health(
        Arc::new(UnusedExecution),
        Arc::new(worker_health),
    )
    .await
    .oneshot(
        Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .expect("health request"),
    )
    .await
    .expect("health response");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn healthz_should_reject_critical_worker_failure() {
    let worker_health = StaticWorkerHealth(vec![worker_snapshot(
        WorkerKind::RuntimeSnapshotReconciliation,
        WorkerRuntimeState::BackingOff,
    )]);
    let response = crate::openai::api_router_with_worker_health(
        Arc::new(UnusedExecution),
        Arc::new(worker_health),
    )
    .await
    .oneshot(
        Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .expect("health request"),
    )
    .await
    .expect("health response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn ready_should_return_no_content_when_all_dependencies_are_healthy() {
    let probes = ["postgres", "redis", "postgres_schema", "runtime_snapshot"]
        .into_iter()
        .map(|name| Arc::new(StaticProbe::healthy(name)) as Arc<dyn HealthProbe>)
        .collect();
    let response = crate::openai::api_router_with_probes(Arc::new(UnusedExecution), probes)
        .await
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .expect("ready request"),
        )
        .await
        .expect("ready response");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn ready_should_return_service_unavailable_when_a_dependency_is_unhealthy() {
    let probes = vec![
        Arc::new(StaticProbe::healthy("postgres")) as Arc<dyn HealthProbe>,
        Arc::new(StaticProbe::unhealthy("redis")),
        Arc::new(StaticProbe::healthy("postgres_schema")),
        Arc::new(StaticProbe::healthy("runtime_snapshot")),
    ];
    let response = crate::openai::api_router_with_probes(Arc::new(UnusedExecution), probes)
        .await
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .expect("ready request"),
        )
        .await
        .expect("ready response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn healthz_should_keep_liveness_when_schema_probe_is_unhealthy() {
    let probes = vec![Arc::new(StaticProbe::unhealthy("postgres_schema")) as Arc<dyn HealthProbe>];
    let router = crate::openai::api_router_with_probes(Arc::new(UnusedExecution), probes).await;
    let healthz = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("health request"),
        )
        .await
        .expect("health response");
    let ready = router
        .oneshot(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .expect("ready request"),
        )
        .await
        .expect("ready response");

    assert_eq!(healthz.status(), StatusCode::NO_CONTENT);
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[derive(Clone)]
struct StaticWorkerHealth(Vec<WorkerHealthSnapshot>);

impl WorkerHealthSource for StaticWorkerHealth {
    fn snapshot(&self) -> Vec<WorkerHealthSnapshot> {
        self.0.clone()
    }
}

fn worker_snapshot(kind: WorkerKind, state: WorkerRuntimeState) -> WorkerHealthSnapshot {
    let id = WorkerId::try_new(kind, "health-test").expect("worker ID");
    WorkerHealthSnapshot {
        key: WorkerHealthKey::Task(id),
        state,
        consecutive_failures: 1,
        completed_cycles: 0,
        last_fencing_token: None,
        last_success_at: None,
        last_failure_at: None,
        last_error: Some("test worker failure".to_owned()),
    }
}

struct StaticProbe {
    name: &'static str,
    state: HealthState,
}

impl StaticProbe {
    fn healthy(name: &'static str) -> Self {
        Self {
            name,
            state: HealthState::Healthy,
        }
    }

    fn unhealthy(name: &'static str) -> Self {
        Self {
            name,
            state: HealthState::Unhealthy("test failure".to_owned()),
        }
    }
}

impl HealthProbe for StaticProbe {
    fn name(&self) -> &'static str {
        self.name
    }

    fn check(&self) -> BoxFuture<'_, HealthState> {
        let state = self.state.clone();
        Box::pin(async move { state })
    }
}

struct UnusedExecution;

impl ExecutionService for UnusedExecution {
    fn authenticate(&self, _: &str) -> Result<AuthenticatedClient, ClientAuthenticationError> {
        unreachable!("health check does not authenticate")
    }

    fn public_models(&self, _: &AuthenticatedClient) -> Vec<PublicModelId> {
        unreachable!("health check does not list models")
    }

    fn contains_public_model(&self, _: &AuthenticatedClient, _: &PublicModelId) -> bool {
        unreachable!("health check does not inspect models")
    }

    fn start(&self, _: StartExecution) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async { unreachable!("health check does not execute requests") })
    }

    fn start_provider_endpoint(
        &self,
        _: StartProviderExecution,
    ) -> BoxFuture<'_, Result<StartedExecution, GatewayError>> {
        Box::pin(async { unreachable!("health check does not execute provider endpoints") })
    }
}
