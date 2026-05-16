# orv

**프로젝트 특화(Project-Specialized) 풀스택 언어 플랫폼**

> orv는 단순한 새 언어가 아니다. 언어, 컴파일러, 에디터, 런타임, 디자인 시스템이 같은 프로젝트 그래프를 공유하는 통합 플랫폼을 목표로 한다.

## 철학

### 북극성 목표: 비개발자가 AI 없이 5시간 안에 쇼핑몰을 만든다

orv의 생산성 목표는 슬로건이 아니라 제품 벤치마크다. 코딩 경험이 거의 없는 사람이 Copilot, Cursor, ChatGPT 같은 AI 보조 없이 결제, 배송, 회원 기능이 있는 작은 쇼핑몰을 5시간 안에 만들고 검증할 수 있어야 한다.

이 목표 때문에 orv는 다음을 우선한다.

- 빌드 도구, 프레임워크 조합, 라이브러리 선택 같은 우발적 복잡성을 줄인다.
- 라우트, DB, 폼, 결제, 배송, 디자인을 도메인 문법으로 표현한다.
- 타입/스키마 선언이 폼 검증, HTTP body 검증, DB schema, migration과 이어지게 한다.
- 에디터와 런타임이 같은 프로젝트 그래프를 보며 source-to-production reveal을 가능하게 한다.
- 안전한 auth/session/csrf/webhook/payment 기본값을 scaffold에 포함한다.

벤치마크 절차는 [BENCHMARK_SHOP_5H.md](BENCHMARK_SHOP_5H.md)에 둔다.

### 진짜 차별점

orv의 차별점은 "새 문법" 자체보다 **프로젝트 그래프를 중심으로 한 도메인 UX**다. 현재 안정화 단계에서는 RC 메모리 모델, 자체 DB 엔진, full native optimizer보다 `@route + @db + @html + @design + editor reveal + deploy smoke`를 압도적으로 잘 만드는 것이 더 중요하다.

## 현재 구현 상태 요약

현재 orv는 Rust workspace 기반 MVP로 구현 중이다.

- `.orv` source load / lex / parse
- import 기반 project loading과 AST ProjectGraph v1
- name resolution / semantic analysis / HIR lowering
- reference tree-walking runtime
- HTTP/1.1 `@server`
- in-memory DB 및 SQLite row JSON reference adapter
- payment/shipping local/file reference adapter와 HTTP reference stub
- HIR origin map과 semantic `contains`/`calls` edge
- `orv graph`, `orv origins`, `orv reveal`
- build/deploy artifact contract
- `orv init <dir> --template shop`
- 일부 editor/LSP/DAP/bootstrap surface

Native optimizer, production editor reveal UI, full project-specialized runtime generation, custom DB engine, CRDT, `@gpu`, `@net`, broad FFI는 아직 안정 제품 계약이 아니다.

상세 판정은 [MVP.md](MVP.md), [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md), [OPERATIONAL_SURFACES.md](OPERATIONAL_SURFACES.md)를 본다.

## 현재 안정화 중심축

```text
ProjectGraph + HIR Origin + Reference Runtime + Trace/Reveal
```

지금 중요한 것은 기능을 더 넓히는 것보다 이 축의 계약을 안정화하는 것이다. `Span -> AST node -> HIR node -> runtime event -> origin id` 연결, `orv graph`/`orv origins`/`x-orv-origin-id`/trace JSON의 같은 origin schema, CLI/static graph view만으로 production output에서 source로 돌아가는 reveal path가 먼저 단단해야 한다.

## MVP 범위

구현 중인 제품 MVP는 "쇼핑몰 작성에 반드시 필요한 20%"에 집중한다.

| 포함 | 뒤로 미룸 |
|------|-----------|
| `@server`, `@route`, `@html`, `@form` | `@gpu`, `@net`, CRDT |
| `@db`, schema/migration DSL | custom DB optimizer, sharding, replication |
| `@auth`, `@session`, `@csrf`, `@rateLimit` | full self-host editor |
| `@payment`, `@shipping`, webhook safety | broad FFI and `@unsafe` workflows |
| `orv init <dir> --template shop`, `orv dev`, `orv build --prod` | full native compiler and optimized client runtime |
| `orv deploy-env-check`, `orv benchmark-report`, generated preflight/benchmark evidence artifacts, generated smoke-test | advanced cloud object storage/provider matrix |

Generated smoke tests are part of the MVP contract: production builds should check reachable server routes, the reference shop checkout/admin flow, and interactive client bundle files/markers before a non-developer treats a build as deployable.

## 통합 플랫폼의 네 레이어

```
┌─────────────────────────────────────────┐
│ Editor    — 프로젝트 그래프의 라이브 뷰 │
├─────────────────────────────────────────┤
│ Language  — 도메인 의도를 문법으로 표현 │
├─────────────────────────────────────────┤
│ Compiler  — 그래프, 검증, 산출물 계약   │
├─────────────────────────────────────────┤
│ Runtime   — reference 실행과 배포 경로   │
└─────────────────────────────────────────┘
```

현재 MVP의 source-to-production 연결은 HIR origin map, `contains`/`calls` edge, HTTP route origin id 헤더, build artifact reveal CLI에서 시작한다. DOM 요소, DB 쿼리, job, trace, 로그에서 에디터로 직접 reveal하는 풍부한 native UI는 로드맵이다.

## 문서 구조와 읽는 순서

| 문서 | 책임 |
|------|------|
| [README.md](README.md) | 비전, 대상 사용자, MVP 경계 |
| [MVP.md](MVP.md) | 지금 되는 것과 MVP non-goal |
| [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md) | 상태, 계약 레벨, milestone, crate, fixture, CLI 표 |
| [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) | 상태 용어와 빠른 요약 |
| [SPEC.md](SPEC.md) | 공식 문법과 목표 의미론 |
| [ARCHITECTURE.md](ARCHITECTURE.md) | 현재 Rust crate 구조와 데이터 흐름 |
| [OPERATIONAL_SURFACES.md](OPERATIONAL_SURFACES.md) | CLI/LSP/DAP/build/DB 운영 surface |
| [BENCHMARK_SHOP_5H.md](BENCHMARK_SHOP_5H.md) | 5시간 쇼핑몰 테스트 프로토콜 |
| [SECURITY_MODEL.md](SECURITY_MODEL.md) | 안전한 기본값과 scaffold 보안 기대치 |
| [ROADMAP.md](ROADMAP.md) | 미래 기능 |
| [CHANGELOG.md](CHANGELOG.md) | 날짜가 붙은 구현 델타 |
| [DOCUMENTATION.md](DOCUMENTATION.md) | 문서 수정 규칙 |

처음 읽을 때는 `README -> MVP -> IMPLEMENTATION_MATRIX -> SPEC -> ARCHITECTURE` 순서를 권장한다. 문서 간 충돌 시 언어 의미론은 `SPEC.md`, 구현/계약 상태는 `IMPLEMENTATION_MATRIX.md`, 운영 surface는 `OPERATIONAL_SURFACES.md`를 따른다.

## 성능 목표

orv의 "Zero-runtime" 원칙은 사용하지 않는 런타임 계층을 번들에 넣지 않는다는 뜻이다. 모든 앱이 0 byte 또는 3 KB가 된다는 뜻은 아니다.

| 앱 유형 | 초기 번들 목표 | 예시 |
|---------|----------------|------|
| 정적 랜딩/블로그 | 0 byte JS | 문서, 마케팅 페이지 |
| 가벼운 대화형 SPA | <= 3 KB JS/WASM shell | 폼, 카운터 |
| 표준 SPA | <= 30 KB initial shell + lazy route | dashboard |
| 그래픽스/미디어 SPA | <= 200 KB shell + streamed assets | design/media tool |
| 게임/네이티브급 | <= 1 MB shell + streamed assets | browser game |

Backend, editor, and compiler performance targets are targets, not published claims. Any external comparison must include hardware, route shape, payload, concurrency, warm/cold mode, TLS, compiler profile, and benchmark harness.

## 프로젝트 구조

```
miol/
├── crates/
│   ├── orv-analyzer
│   ├── orv-cli
│   ├── orv-compiler
│   ├── orv-core
│   ├── orv-diagnostics
│   ├── orv-hir
│   ├── orv-ids
│   ├── orv-macros
│   ├── orv-project
│   ├── orv-resolve
│   ├── orv-runtime
│   └── orv-syntax
├── docs/
│   ├── README.md
│   ├── MVP.md
│   ├── IMPLEMENTATION_MATRIX.md
│   ├── IMPLEMENTATION_STATUS.md
│   ├── SPEC.md
│   ├── ARCHITECTURE.md
│   ├── OPERATIONAL_SURFACES.md
│   ├── BENCHMARK_SHOP_5H.md
│   ├── SECURITY_MODEL.md
│   ├── ROADMAP.md
│   ├── CHANGELOG.md
│   └── DOCUMENTATION.md
└── fixtures/
    ├── default-syntax.orv
    ├── e2e/
    └── plan/
```

## 기술 스택

- Rust edition 2021, MSRV 1.86.0
- wgpu 29
- codespan-reporting 0.11
- serde + serde_json
- clap 4

## 빌드

```bash
rtk cargo build
rtk cargo test
rtk cargo clippy
```

## 라이선스

MIT
