# Rust backend workspace

This repository contains a production-oriented Rust backend foundation built with Axum. It connects to MySQL through SeaORM, Redis through a reconnecting asynchronous client, and OpenSearch through the official Rust client.

The API uses feature-based vertical slices, validates environment configuration at startup, emits structured tracing, and provides liveness and dependency-readiness endpoints. MySQL, Redis, and OpenSearch are available locally through Docker Compose.

## Repository layout

```text
.
|-- .github/workflows/   Backend continuous-integration workflow
|-- backend/             Rust workspace, containers, migrations, and backend docs
|-- AGENTS.md            Canonical coding-agent and repository instructions
|-- CLAUDE.md            Claude entry point that defers to AGENTS.md
|-- LICENSE              MIT license
`-- README.md            Repository overview
```

The backend workspace contains:

```text
backend/
|-- crates/api/          Axum API with feature-based vertical slices
|-- migration/           SeaORM migration CLI and migration registry
|-- scripts/             Docker-backed integration-test harness
|-- compose.yaml         MySQL, Redis, OpenSearch, and optional API service
|-- Dockerfile           Multi-stage backend image
`-- README.md            Complete backend development documentation
```

## Quick start

Requirements:

- Rust 1.92 or newer
- Docker Desktop or Docker Engine with Compose
- PowerShell, Bash, or another shell capable of setting environment variables

From the repository root in PowerShell:

```powershell
Set-Location backend
Copy-Item .env.example .env
docker compose up --detach --wait mysql redis opensearch
cargo run -p migration -- up
cargo run -p backend-api
```

From Bash:

```bash
cd backend
cp .env.example .env
docker compose up --detach --wait mysql redis opensearch
cargo run -p migration -- up
cargo run -p backend-api
```

Once running, the API exposes:

- `GET http://127.0.0.1:3000/health/live` for process liveness.
- `GET http://127.0.0.1:3000/health/ready` for MySQL, Redis, and OpenSearch readiness.

See [backend/README.md](backend/README.md) for configuration, architecture, migrations, Docker usage, troubleshooting, and all development commands.

## Development checks

Run backend checks from `backend/`:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
docker compose config --quiet
```

Run the real datastore integration suite with:

```bash
bash scripts/integration-test.sh
```

The integration harness uses an isolated Compose project and removes its own containers, network, and ephemeral volumes when it finishes.

## Continuous integration

The backend workflow runs on pushes and pull requests that affect backend code or the workflow itself. It gates changes on formatting, strict Clippy, compile checks, a complete build, unit tests, and Docker-backed integration tests.

Workflow: [.github/workflows/backend-ci.yml](.github/workflows/backend-ci.yml)

## Agent-assisted development

Coding agents must follow [AGENTS.md](AGENTS.md). Claude-based tools should begin with [CLAUDE.md](CLAUDE.md), which points to the same canonical instructions.

## License

This repository is licensed under the [MIT License](LICENSE).

