# docker-observer-proxy

A small, allowlisted, read-only Docker API view for
[Homepage](https://gethomepage.dev/configs/docker/). It exposes only the three
non-Swarm operations Homepage 2.1.2 uses for discovery, status and statistics:

- `GET /containers/json?all=true`
- `GET /containers/{allowed-name}/json`
- `GET /containers/{allowed-name}/stats?stream=false`

`GET /_ping` and `GET /version` are also available for Docker client
compatibility. Every mutating method and every other Docker endpoint is denied.
Container-list responses are filtered, so an authenticated client cannot even
inventory containers outside `DOP_ALLOWED_CONTAINERS`.

This is not a general Docker proxy. In particular it does not expose logs,
exec, archives, images, volumes, secrets, events or system information.

## Why

Homepage runs on `hmsrv01-observe-01` (`10.83.100.10`), while the applications
it should display run under Docker on `hmsrv01` (`10.83.100.5` on VLAN 100).
Mounting a local socket into Homepage would inspect the wrong machine. Exposing
Docker's raw API over TCP would make a dashboard credential equivalent to root
on `hmsrv01`.

This proxy provides the narrow API shape Homepage needs and nothing more. It
does not make Docker's socket harmless: the proxy process still holds a
root-equivalent capability, so keep the binary minimal, pin its image, restrict
the listener at the host firewall, and treat its token as a secret.

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `DOP_LISTEN_ADDR` | `127.0.0.1:2375` | Exact address and port to bind |
| `DOP_DOCKER_SOCKET` | `/var/run/docker.sock` | Docker Unix socket |
| `DOP_ALLOWED_CLIENT_IPS` | `127.0.0.1,::1` | Exact client IP allowlist |
| `DOP_ALLOWED_CONTAINERS` | required | Comma-separated Docker container names |
| `DOP_AUTH_TOKEN` | unset | Bearer token, at least 32 bytes |
| `DOP_AUTH_TOKEN_FILE` | unset | Preferred file containing the bearer token |
| `DOP_TIMEOUT_SECONDS` | `5` | Complete upstream request deadline |
| `DOP_MAX_RESPONSE_BYTES` | `8388608` | Maximum Docker response body |
| `DOP_LOG_FORMAT` | compact | Set to `json` for structured production logs |
| `RUST_LOG` | `docker_observer_proxy=info` | Standard tracing filter |

Set exactly one of `DOP_AUTH_TOKEN` and `DOP_AUTH_TOKEN_FILE`. The latter keeps
the token out of `docker inspect` and systemd's environment display.

Generate a token with:

```bash
openssl rand -base64 48
```

## Docker Compose

Copy `docker-compose.yml`, create `secrets/docker-observer-token`, and set the
container names plus the Docker socket's group ID:

```bash
install -d -m 0700 secrets
openssl rand -base64 48 > secrets/docker-observer-token
chmod 0600 secrets/docker-observer-token
export DOCKER_GID=$(stat -c '%g' /var/run/docker.sock)
export DOP_ALLOWED_CONTAINERS='jellyfin,sonarr,radarr'
docker compose up -d
```

The example uses host networking so application-level source-IP checks see
`10.83.100.10` rather than a Docker bridge address. Add a host firewall rule
that permits TCP/2375 only from `10.83.100.10`; the in-process check is defence
in depth, not a substitute for that rule.

## systemd

Install the release binary at `/usr/local/bin/docker-observer-proxy`, then use
the files under `deploy/`:

```bash
sudo useradd --system --home-dir /nonexistent --shell /usr/sbin/nologin \
  --groups docker docker-observer-proxy
sudo install -m 0755 docker-observer-proxy /usr/local/bin/
sudo install -m 0644 deploy/docker-observer-proxy.service /etc/systemd/system/
sudo install -m 0600 deploy/docker-observer-proxy.env.example \
  /etc/docker-observer-proxy.env
sudo install -m 0600 /path/to/token /etc/docker-observer-proxy.token
sudo systemctl daemon-reload
sudo systemctl enable --now docker-observer-proxy
```

Membership in the `docker` group is root-equivalent. The unit hardening limits
accidental access but cannot change that underlying Docker security property.

## Homepage

Start from `deploy/homepage-docker.yaml.example`. Put the matching token in
Homepage's secret environment as `HOMEPAGE_VAR_DOCKER_PROXY_TOKEN`, then add
`server` and `container` to each desired service:

```yaml
- Media:
    - Jellyfin:
        href: https://jellyfin.example.org
        server: hmsrv01
        container: jellyfin
        showStats: true
```

The proxy and Homepage allowlists should name the same containers. A mismatch
fails closed with a not-found response.

## Development

Rust 2024 edition with MSRV 1.85:

```bash
cargo fmt --all --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
docker build -t docker-observer-proxy:dev .
```

The test suite concentrates on the security contract: exact Homepage routes,
method denial, streaming denial, path-smuggling rejection, container filtering,
and bearer authentication. It intentionally avoids a large mock-Docker
framework.

## Releases

Push a semantic version tag such as `v0.1.0`. The release workflow:

1. formats, tests and lints;
2. builds static x86-64 and ARM64 Linux archives with checksums;
3. publishes a multi-architecture image to
   `ghcr.io/caddyglow/docker-observer-proxy`;
4. creates a GitHub release with both binary archives.

Cargo and Docker BuildKit caches are reused across runs. No release is made
from an untagged commit. Release binaries are compiled once and reused by the
multi-architecture image build; ARM64 is not recompiled under QEMU.

See [PLAN.md](PLAN.md) for the threat model, rollout sequence and acceptance
criteria.
