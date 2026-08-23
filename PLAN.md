# Implementation and rollout plan

## Outcome

Homepage on `hmsrv01-observe-01` can display discovery, health and resource
statistics for an explicit subset of containers on `hmsrv01`, without mounting
the raw Docker socket into Homepage or exposing a general Docker API on the
network.

The implementation stays deliberately small. It is a typed allowlist in front
of a Unix socket, not a configurable reverse-proxy platform.

## Proven client contract

The initial policy was derived from the pinned Homepage `v2.1.2` source rather
than from the full Docker API. In non-Swarm mode it calls:

1. `listContainers({ all: true })` for label discovery and before status/stats;
2. `getContainer(name).inspect()` for status and health;
3. `getContainer(name).stats({ stream: false })` for CPU, memory and network.

That maps to three Docker routes. `_ping`, `/version`, and optional `/v1.NN`
prefixes are retained for client compatibility. Swarm services and tasks are
explicitly out of scope because `hmsrv01` uses Compose.

Before upgrading Homepage, run it against proxy logs in staging. An upgrade
that needs a new Docker route must come with a narrowly scoped policy change
and tests; never add a wildcard compatibility escape hatch.

## Threat model

Protect against:

- a Homepage compromise trying to create, exec into, stop or delete containers;
- a stolen token inventorying containers unrelated to the dashboard;
- a LAN client reaching the proxy without being the observability VM;
- path encoding, duplicate query keys or streaming stats broadening a request;
- a slow or oversized Docker response consuming the proxy indefinitely;
- credentials leaking through request logs.

Not solved here:

- code execution inside the proxy process, which already has Docker socket
  access and must therefore be treated as root-equivalent;
- a compromised Docker daemon returning malicious but valid JSON;
- confidentiality against packet capture on VLAN 100. Move the link to
  Tailscale or add TLS before crossing an untrusted network.

## Security contract

- Exact source-IP allowlist and bearer token are both mandatory in production.
- Only GET is accepted, except HEAD on `_ping`.
- Only list, inspect and non-streaming stats are exposed for explicitly named
  containers.
- Container lists are parsed and filtered before returning them.
- Authorization and hop-by-hop client headers are never forwarded to Docker.
- Unknown paths return 404; mutations return 405; authentication failures are
  indistinguishable beyond 401.
- Upstream work has a five-second deadline and an 8 MiB response ceiling by
  default.
- Logs contain client IP, method, path, status and duration, never headers,
  response bodies or tokens.

## Project phases

### 1. Core implementation — complete

- [x] Rust 2024 project with MSRV 1.85 and no unsafe code
- [x] Axum HTTP listener and Hyper HTTP/1 client over a Unix socket
- [x] Environment/file configuration with startup validation
- [x] SHA-256 plus constant-time bearer-token comparison
- [x] Exact Docker method/path/query policy
- [x] Container-name allowlist and list-response filtering
- [x] Bounded response collection and complete request timeout
- [x] Compact local logs and JSON production logs through `tracing`
- [x] Graceful SIGINT/SIGTERM shutdown

### 2. Focused verification — complete

- [x] Permit the three Homepage v2.1.2 non-Swarm calls
- [x] Support version-prefixed Docker paths
- [x] Reject POST, PUT, PATCH and DELETE
- [x] Reject logs, archives, images, volumes, info and unknown routes
- [x] Reject streaming stats, extra query keys and encoded/smuggled paths
- [x] Reject unlisted container names
- [x] Filter unlisted containers and fail closed on malformed Docker JSON
- [x] Verify exact bearer authentication
- [x] `cargo fmt`, tests and Clippy with warnings denied

The suite remains unit-focused. A mock implementation of the whole Docker API
would add more test code than production code without strengthening this narrow
policy. Live compatibility is covered by the rollout probe below.

### 3. Packaging and releases — complete

- [x] Cached multi-stage Dockerfile with a scratch runtime image
- [x] Hardened Compose example using host networking and a secret file
- [x] Hardened systemd unit and environment template
- [x] CI on pull requests and `main`
- [x] Tag-only release workflow for static x86-64/ARM64 binaries, checksums,
      GitHub releases and a cached multi-architecture GHCR image; the image
      reuses those binaries rather than recompiling ARM64 under QEMU

### 4. hmsrv01 deployment — pending

This belongs in `CaddyGlow/homelab-compose`, which owns the Docker host.

- [ ] Store a 48-byte random token in Infisical; do not commit it
- [ ] Choose the initial container names and set `DOP_ALLOWED_CONTAINERS`
- [ ] Deploy the pinned GHCR digest on host networking
- [ ] Publish only `10.83.100.5:2375`
- [ ] Add a host firewall rule allowing TCP/2375 only from `10.83.100.10`
- [ ] Confirm another VLAN 100 host cannot connect
- [ ] Confirm restart and resource limits behave under Docker failure

Do not deploy `latest` in the live Compose tree. The README uses it for a
copyable example; production must pin the tag and resolved digest.

### 5. Homepage integration — pending

This belongs in `CaddyGlow/mazenet-infra`, which owns the Homepage role.

- [ ] Add `HOMEPAGE_DOCKER_PROXY_TOKEN` to Infisical
- [ ] Render `docker.yaml` with `10.83.100.5:2375` and the Authorization header
- [ ] Add the token to `homepage.env` as a `HOMEPAGE_VAR_*` substitution
- [ ] Mount `docker.yaml` with the existing config files
- [ ] Add matching `server`, `container`, and selective `showStats` fields
- [ ] Keep the existing no-socket mount: Homepage never receives Docker access
- [ ] Update the Homepage role README with the two-repository ownership split

### 6. Rollout and acceptance — pending

From `hmsrv01-observe-01`, using the real token:

- [ ] `_ping` returns success
- [ ] `/containers/json?all=true` contains every configured container and no
      unconfigured container
- [ ] inspect reports state/health for each configured name
- [ ] stats returns one finite JSON response and closes
- [ ] Homepage shows status and expanded CPU/memory/network values
- [ ] a wrong token returns 401
- [ ] an unlisted container returns 404
- [ ] POST `/containers/create` returns 405
- [ ] logs, exec, archive, image, volume, event and info routes are unavailable
- [ ] stopping Docker produces a bounded 502 rather than a hung Homepage request
- [ ] proxy logs contain no token, Docker body, label value or environment value

Roll out with one low-risk container first. Add the remaining names only after
Homepage status and stats remain stable for a day.

## Maintenance rule

The allowlist is the product. Any feature that turns this into a generic Docker
API proxy is a rejection, not an enhancement. If a future dashboard needs a
substantially wider API, deploy a purpose-built agent that returns a dedicated
metrics schema instead of weakening this boundary.
