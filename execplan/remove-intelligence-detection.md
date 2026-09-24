# Remove Intelligence Detection

This ExecPlan is a living document. The `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` sections must stay current while the change is implemented.

## Purpose / Big Picture

Remove the intelligence/degradation detection feature from the fork so it no longer exposes detection settings, records, admin routes, or a background detection worker. Existing account probing, manual scheduling suspension, and the separate quota-reset detection feature must continue to work. Existing database migrations, tables, and records remain because deployed PostgreSQL instances verify migration checksums at startup.

After the change, the management UI has no detection navigation or settings card, the backend does not register or run `IntelligenceDetectionTask`, and the detection admin endpoints are absent. The service still starts against the existing database and preserves the reset-detection settings and account scheduling controls.

## Progress

- [x] (2026-09-24 13:00 +08:00) Audited frontend, backend, storage, migrations, worker registration, and shared scheduling boundaries.
- [x] (2026-09-24 13:00 +08:00) Created branch `fix/remove-intelligence-detection` from the fork main branch.
- [x] (2026-09-24 14:05 +08:00) Removed frontend detection routes, settings, navigation, store state, API module, view helpers, and detection-only tests while preserving reset detection and account scheduling.
- [x] (2026-09-24 14:05 +08:00) Removed backend detection routes, use cases, worker registration, provider operation, task kind, store adapter wiring, and detection-only tests while preserving shared account probing and manual suspension.
- [x] (2026-09-24 14:05 +08:00) Updated product/release documentation without changing historical migrations or deleting deployed detection data.
- [x] (2026-09-24 17:00 +08:00) Frontend tests passed (2 files, 28 tests), frontend production build passed, Rust formatting passed, and the migration freeze/checksum manifest passed including migration 0012.
- [ ] Rust workspace tests remain unverified on this Mac: installed rustc is 1.95, while the workspace requires 1.97. A prior remote image build was reported successful, but its test log is unavailable in this resumed session.
- [ ] Push the branch and create/refresh its PR after GitHub authentication is restored. `gh auth status` reports the configured token invalid; no remote branch exists yet.
- [ ] Deploy the updated fork image to the existing production Compose stack only after PR/CI and rollback checks. SSH to `mac@38.129.137.230` currently fails public-key authentication. Live `/healthz` and `/ready` return 204, but this only verifies the existing service.

## Surprises & Discoveries

- The production instance uses the `yuzaicn/codex-proxy-rs` fork, not the upstream `zyycn/codex-proxy-rs` repository. The fork main commit used for the current deployment is `0cf2e114…`.
- GitHub `main` still points to `0cf2e114a5a4ec08bf02909f3626bb09a43c19cc`, matching this branch's base. Open PR #41 proposes a different migration numbered `0012`, and open PR #42 proposes replacing intelligence detection with ModelTrace; both conflict in direction or migration numbering and need maintainer coordination before integration.
- Detection migrations `0006` through `0011` are already part of the deployed schema and are checksum-verified. Removing or editing those files would prevent startup; they remain historical compatibility artifacts.
- `0008_reset_detection.sql`, reset-detection settings, ordinary account probes, and manual scheduling suspension are separate or shared functionality and must not be removed.
- `scheduling_suspended` and `scheduling_suspended_by` are shared account state. The detection worker's writes will disappear, but the columns and manual source remain for compatibility.

## Decision Log

- Decision: Remove detection runtime code and public contracts, but keep historical migrations, tables, columns, and records.
  Rationale: deployed SQLx migration checksums and existing audit data require schema history to remain stable; the user's request is to remove the feature's operation and UI.
  Date/Author: 2026-09-24 / Codex.
- Decision: Do not remove `SchedulingSuspensionSource::Detection` or `AccountSchedulingBlocker::SuspendedByDetection` in this change.
  Rationale: historical rows may contain `detection`; removing enum variants would create compatibility failures and is not required to stop new detection work.
  Date/Author: 2026-09-24 / Codex.

## Outcomes & Retrospective

The application-level intelligence-detection UI, API, worker, and storage wiring are removed. Historical migrations and detection audit data remain, as do separate reset-card detection, normal account connection tests, and manual scheduling suspension. Frontend tests/build and Rust formatting/migration checksum checks passed. Rust tests and production deployment are still pending due the current toolchain/authentication blockers above; no production state was changed.

## Context and Orientation

The repository is a Rust workspace under `backend/` with an Axum admin API, an admin service crate, a PostgreSQL adapter, and a Vue frontend under `frontend/`. Intelligence detection is the scheduled feature that probes provider accounts for degraded output, stores detection rounds/records, and may suspend scheduling. Its backend entry points are `backend/crates/gateway-api/src/admin/detection.rs`, `backend/crates/gateway-admin/src/use_case/detection.rs`, `backend/crates/gateway-admin/src/workers/intelligence_detection.rs`, and `backend/crates/gateway-store/src/postgres/detection.rs`. Its frontend entry points are `frontend/src/views/detection/`, `frontend/src/views/settings/components/DetectionSettingsCard.vue`, `frontend/src/api/modules/detection.ts`, and the detection route/sidebar wiring.

The reset-detection feature is different: it controls active quota reset cards and lives in the reset-detection settings components and `backend/migrations/0008_reset_detection.sql`. Account probing is also shared by ordinary account connection tests. A safe removal must leave those paths compiling and tested.

## Plan of Work

First remove frontend-only detection surfaces: the `/detection` and `/settings/detection` routes, sidebar conditional and config fetch, detection settings segment/card, detection store state, detection API export, view helpers, and detection-only tests. Keep reset-detection imports, route, settings card, and the account page scheduling controls.

Next remove backend-only detection surfaces. Unmount the admin detection router, delete the detection use-case/model and worker construction/registration, remove the worker kind from scheduling and health fixtures, remove the `DetectionStore` adapter and its port bundle wiring, and remove the ProviderAdmin operation that exists only to construct detection requests. Adjust compile-time tests and mocks for the reduced traits. Keep the database migration files and shared scheduling enum variants for persisted-data compatibility.

Finally update product/release documentation to stop advertising the removed feature, run targeted frontend tests and Rust workspace tests, run migration checksum validation, and build the release image. A production deployment must create a database backup and Compose rollback copy first, replace only the application image, and verify health, login, version, and 404 responses for removed detection routes.

## Concrete Steps

Run all commands from `/Users/mac/Documents/Codex/2026-09-24/gen/work/codex-proxy-rs-yuzaicn`.

1. Search all remaining references with `rg -n -i 'intelligence.?detection|detectionEnabled|降智|智能检测|degraded' backend frontend PRODUCT.md release` and classify each hit as detection-only, reset-detection, generic health degradation, or shared scheduling.
2. Apply the frontend and backend edits in their owned files. Do not edit `backend/migrations/*.sql` or `.frozen-sha256`.
3. Run `pnpm --dir frontend test --run` and `pnpm --dir frontend build`.
4. Run `cargo test --workspace` and `cargo build --workspace` from `backend/`, plus `sha256sum --check --strict backend/migrations/.frozen-sha256`.
5. Build the `linux/amd64` runtime image from the validated branch, back up PostgreSQL and the production Compose file, then recreate only the app container.
6. Verify direct and public `/healthz` and `/ready` as 204, admin login and version as 200, `/api/admin/detection/config` as 404, no detection sidebar/settings in the rendered UI, and zero app restarts during an observation window.

## Validation and Acceptance

The change is accepted when frontend and Rust tests/builds pass, migration checksums pass, no active runtime/API/UI reference to intelligence detection remains, and reset-detection plus normal account probing tests remain green. The service must start against the existing database without migration checksum errors. In production, the app container must be healthy with restart count zero; the existing PostgreSQL and Redis containers must remain healthy; login and version endpoints must work; removed detection endpoints must return 404; and the rollback Compose copy and database backup must be present.

## Idempotence and Recovery

Source edits are repeatable on the task branch. Historical migrations must never be rewritten or deleted. Before production replacement, save the current Compose file and a PostgreSQL custom-format dump. If the new app fails health, restore the Compose file and recreate only the app container with the previous image. Do not delete `.runtime` directories or alter PostgreSQL/Redis data during rollback.

## Artifacts and Notes

Expected local evidence includes a clean migration checksum check, successful frontend/Rust test commands, a source search showing only historical migration and explicitly retained compatibility references, and a healthy post-deployment container inspection. Sensitive configuration, credentials, and database contents must not be copied into this plan or command output.

## Interfaces and Dependencies

The remaining admin service must still expose ordinary account management and manual scheduling APIs. The remaining provider admin trait must still support the existing account connection/test operations. The remaining task registry must continue to include `ResetDetection` and other workers while excluding `IntelligenceDetection`. The PostgreSQL migration runner must continue to see every existing migration file with its original bytes.
