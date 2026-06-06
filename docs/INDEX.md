# VIL Documentation

**Version:** v0.4.0
**License:** Apache-2.0
**GitHub:** https://github.com/OceanOS-id/VIL

---

## The Two Components

VIL is organized into two components with distinct roles:

| Component | Role | Binary |
|-----------|------|--------|
| **[VIL](./vil/)** | Intermediate language (macros, semantics, zero-copy runtime) | Library crates |
| **[vil-server](./vil-server/)** | Standalone compiled server (multi-service, Tri-Lane) | `cargo build` → binary |

```
VIL Source Code
    │
    └── cargo build ──────→ vil-server (standalone binary)
                              Compile-time multi-service
                              Tri-Lane SHM inter-service mesh
```

---

## Quick Start

### Just Getting Started?
1. **[Quick Start](./QUICK_START.md)** — Build your first pipeline (10 min)
2. **[Installation](./INSTALLATION.md)** — Setup for Linux, macOS, Docker
3. **[Examples](./EXAMPLES.md)** — 119 runnable examples

### VIL (Language)
- **[VIL Concept](./vil/VIL_CONCEPT.md)** — 10 immutable design principles
- **[Developer Guide (11 parts)](./vil/001-VIL-Developer_Guide-Overview.md)** — complete language reference
- **[Architecture Overview](./ARCHITECTURE_OVERVIEW.md)** — layered system design

### vil-server (Standalone)
- **[Developer Guide](./vil-server/vil-server-guide.md)** — full feature reference
- **[API Reference](./vil-server/API-REFERENCE-SERVER.md)** — per-module docs

### Observability
- **[Observer Dashboard](./vil/010-VIL-Developer_Guide-Observer-Dashboard.md)** — embedded dashboard, SLO budget, alerting, Prometheus export

### Reference
- [SDK Integration](./vil/SDK-Integration-Guide.md) — embedding VIL
- [Changelog](./CHANGELOG.md) — release history
- [Contributing](./CONTRIBUTING.md) — how to help

---

## Documentation by Audience

### Developer (Write Services)
| Start Here | Then | Deep Dive |
|-----------|------|-----------|
| [Quick Start](./QUICK_START.md) | [Examples](./EXAMPLES.md) | [Developer Guide (11 parts)](./vil/001-VIL-Developer_Guide-Overview.md) |

### Ops (Deploy Services)
| vil-server |
|-------------|
| `cargo build` → run binary |

### Architect (Design Systems)
| Document | Focus |
|----------|-------|
| [VIL Concept](./vil/VIL_CONCEPT.md) | Design principles |
| [Architecture](./ARCHITECTURE_OVERVIEW.md) | System layers |

---

**Version:** v0.4.0 | **License:** Apache-2.0 | **Tests:** 1612 | **Crates:** 172
