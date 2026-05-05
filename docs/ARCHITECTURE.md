# orv 아키텍처

## 개요

orv는 Rust workspace로 구성된 11개 크레이트의 파이프라인 아키텍처를 따른다. 현재 구현은 `.orv` 소스를 로드/파싱/해석/분석한 뒤 HIR을 레퍼런스 tree-walking 런타임으로 실행하는 MVP다. `orv-compiler`는 HIR 기반 origin map, build/deploy artifact contract, native server plan/source contract를 생성하고, `orv-runtime`은 HTTP/1.1 `@server`, in-memory DB, request trace JSON writer를 제공한다. 세부 CLI/LSP/DAP/build/DB 운영 surface는 [OPERATIONAL_SURFACES.md](OPERATIONAL_SURFACES.md)에 분리해 추적한다.

이 문서는 **현재 구현 구조와 데이터 흐름**을 설명하는 문서다. 언어 문법과 의미론의 공식 기준은 `docs/SPEC.md`이며, CLI/LSP/DAP/build/DB 운영 세부는 `docs/OPERATIONAL_SURFACES.md`가 담당한다.

소스 위치 타입(`Span`, `ByteRange`)은 별도 크레이트 대신 `orv-diagnostics`에 통합되어 진단 메시지와 함께 관리된다. 바인딩 식별자(`NameId`)는 의존성 방향을 좁히기 위해 무의존 `orv-ids`에 둔다.

## 컴파일 파이프라인

```
  orv-cli (`run` / `check` / `dump` / `origins` / `graph` / `build` / `*-artifact`)
      │
      ▼
┌──────────────┐
│ orv-project  │  파일/Import 로드 + ProjectGraph v1
│ + orv-syntax │  각 파일 lex/parse
└──────┬───────┘
       │ 병합된 AST Program + source map + ProjectGraph v1
      │
      ▼
┌─────────────┐     ┌──────────────────────┐
│ orv-resolve │────▶│ 이름 해석 결과        │
│ 스코프 분석  │     │ (바인딩, 스코프 연결)  │
└─────────────┘     └──────────────────────┘
      │
      ▼
┌──────────────┐     ┌─────────────────┐
│ orv-analyzer │────▶│  HIR (고수준 IR) │
│ 의미 분석     │     └─────────────────┘
└──────────────┘
      │
      ▼
┌──────────────┐     ┌──────────────────────────┐
│ orv-runtime  │────▶│ 레퍼런스 실행 결과        │
│ 인터프리터    │     │ (`@server`는 HTTP/1.1)   │
└──────────────┘     └──────────────────────────┘
```

진단/위치 정보(`orv-diagnostics`)와 공유 식별자(`orv-ids`)는 파이프라인의 여러 단계가 공통으로 사용한다. `orv-core`/`orv-macros`는 공유 인프라로 별도 단계 없이 여러 크레이트가 참조한다. `orv-compiler`는 현재 origin map과 build manifest artifact 생성을 담당하며, 향후 최적화/번들링 단계가 들어갈 자리다.

## 크레이트 상세

| 크레이트 | 역할 | 주요 의존성 |
|----------|------|------------|
| `orv-diagnostics` | 소스 위치(`Span`, `ByteRange`) + 구조화된 컴파일러 진단 메시지. codespan-reporting 기반 포매팅 | codespan-reporting |
| `orv-ids` | 이름 해석이 부여하고 HIR/컴파일러/런타임이 공유하는 compact ID 타입 | — |
| `orv-macros` | Rust proc-macro 유틸리티. 컴파일러 내부에서 사용하는 derive 매크로 | syn, quote, proc-macro2 |
| `orv-core` | 핵심 타입 정의와 공유 인프라. 모든 크레이트가 공통으로 사용하는 타입 | orv-macros, orv-diagnostics |
| `orv-syntax` | 렉서(Lexer)와 파서(Parser). `.orv` 소스를 AST로 변환 | orv-diagnostics |
| `orv-resolve` | 이름 해석(Name Resolution)과 스코프 분석. AST의 식별자를 선언에 연결 | orv-diagnostics, orv-ids, orv-syntax |
| `orv-hir` | 고수준 중간 표현(HIR) 정의. 의미 분석 이후의 타입 정보와 origin id 계산 규칙이 포함된 IR | orv-diagnostics, orv-ids |
| `orv-analyzer` | 의미 분석(Semantic Analysis)과 HIR 로우어링. 타입 검사, 도메인 검증 | orv-diagnostics, orv-hir, orv-resolve, orv-syntax |
| `orv-project` | entry 파일에서 import를 따라 멀티파일 프로그램을 로드/병합하고, 파일/import/선언/domain 기반 AST ProjectGraph v1을 추출 | orv-syntax, orv-diagnostics, thiserror, serde |
| `orv-compiler` | HIR origin map, build manifest/bundle plan, native server plan/package/source/command contract 생성. 코드 생성/최적화 단계는 로드맵 | orv-diagnostics, orv-hir, serde |
| `orv-runtime` | 레퍼런스 tree-walking 런타임. `@server`는 hyper HTTP/1.1 서버로 실행하며 route 응답에 `x-orv-origin-id`를 붙이고, `@db.connect "sqlite://..."` reference adapter는 SQLite 파일에 row JSON을 지속한다 | orv-diagnostics, orv-hir, orv-syntax, serde, serde_json, regex, rusqlite, thiserror, tokio, hyper |
| `orv-cli` | CLI 프론트엔드. 프로젝트 scaffold, source-entry 명령, graph/origin 출력, editor/LSP/DAP bootstrap, build/deploy artifact workflow, DB workflow를 오케스트레이션. 상세 command/method surface는 `docs/OPERATIONAL_SURFACES.md`에서 관리 | orv-diagnostics, orv-syntax, orv-resolve, orv-analyzer, orv-hir, orv-compiler, orv-project, orv-runtime, clap |

DAP bootstrap은 `orv-cli` 안에서 프로젝트 로더, AST/ProjectGraph, reference runtime debug trace를 재사용한다. 외부 editor/debug protocol의 상세 method surface와 attach/trace 운영 계약은 [OPERATIONAL_SURFACES.md](OPERATIONAL_SURFACES.md)에 둔다.

## 의존성 그래프

```
orv-cli
├── orv-project ──▶ orv-syntax ──▶ orv-diagnostics
├── orv-resolve ──▶ orv-syntax / orv-diagnostics / orv-ids
├── orv-analyzer ─▶ orv-hir / orv-resolve / orv-syntax / orv-diagnostics
├── orv-runtime ──▶ orv-hir / orv-syntax / orv-diagnostics
└── orv-compiler ─▶ orv-hir / orv-diagnostics / serde

orv-core ──▶ orv-macros / orv-diagnostics
orv-hir ──▶ orv-diagnostics / orv-ids
```

## 데이터 흐름

### 0단계: 프로젝트 로드 (orv-project)

```
entry .orv → import DFS → 병합된 AST Program + source map + ProjectGraph v1
```

entry 파일에서 시작해 `import` 문을 따라 `.orv` 파일을 재귀적으로 로드한다. 현재는 import 된 파일의 top-level 문장을 entry 앞에 붙여 하나의 AST `Program`으로 병합하고, `LoadedProject`에 파일별 source map과 AST 기반 `ProjectGraph`를 함께 담는다. 같은 병합 규칙은 파일 시스템 로드와 build artifact source bundle 재수화 모두에 적용된다. ProjectGraph v1은 `File`, `Import`, `Struct`, `Enum`, `TypeAlias`, `Function`, `Define`, `Domain` 노드와 `Contains`, `Imports` 엣지를 제공한다. 파일별 scope 격리, visibility enforcement, 외부 레지스트리 의존성, 정교한 사이클 진단은 로드맵이다.

### 0.5단계: 프로젝트 그래프 v1 (orv-project)

```
AST Program + source map → ProjectGraph v1
```

현재 `orv-project`는 AST와 source map만으로 멀티파일 프로젝트의 구조 그래프를 만든다. 이 그래프는 파일, import, 선언, domain 경계를 표현하고, 파일이 포함하는 노드와 import 대상 파일을 연결한다. `orv graph <file>`은 이 구조 그래프와 HIR origin map을 함께 JSON으로 출력하고, `--view --out <dir>`이면 같은 데이터를 `graph.json`과 search/kind filter가 있는 정적 `index.html` graph view로 쓴다. `stats`에는 node/edge/file/import/declaration/domain count, source `contains` 최대 깊이, semantic origin/edge/call count, semantic `contains` 최대 깊이를 담는다. exact span 매칭이 가능한 origin에는 `semantic.origin_links`로 AST node id를 붙인다. 또한 `orv-compiler`가 생성한 HIR origin edge를 `semantic.origin_edges`로 노출한다. 현재 edge는 traversal parent stack 기반 `contains`와 call expression에서 resolved function으로 이어지는 `calls`를 포함하므로, `server -> route -> respond` 같은 의미 실행 계층과 `call -> function` 호출 관계를 볼 수 있다. 따라서 route/respond/function/call 같은 의미 실행 노드와 원본 구조 노드를 같은 artifact에서 볼 수 있다. Workspace graph path dependency edge는 target member name/version과 requested version을 기록하고 version mismatch를 거부한다. CLI 진단도 source map을 사용해 import된 파일의 `FileId`와 실제 경로를 맞춰 출력한다.

### 1단계: 파싱 (orv-syntax)

```
.orv 소스 텍스트 → 토큰 스트림(Lexer) → AST(Parser)
```

소스 텍스트를 토큰으로 분해한 뒤 구문 트리(AST)를 생성한다. 모든 노드에 `Span` 정보가 부착되어 에러 보고 시 정확한 위치를 가리킨다.

### 2단계: 이름 해석 (orv-resolve)

```
AST → 스코프 테이블 + 바인딩 맵
```

식별자를 선언에 연결하고 스코프 계층을 구성한다. `import`/`pub` 가시성 규칙을 적용한다.

### 3단계: 의미 분석 (orv-analyzer)

```
AST + 바인딩 맵 → HIR
```

타입 검사, 도메인 유효성 검증, 스키마 제약조건 확인을 수행한다. 결과를 HIR(고수준 중간 표현)로 로우어링한다.

### 4단계: 레퍼런스 실행 (orv-runtime)

```
HIR → tree-walking 실행
```

현재 런타임은 HIR을 직접 평가한다. 일반 표현식, 함수, 타입/캐스트, HTML 값, 서버 라우트, 인메모리 `@db`, 명시적 `@db.save/load` JSON snapshot, `@db.wal(path)` JSONL append+fsync WAL replay with `ts_unix_ms` record timestamps, `@db.checkpoint()` WAL snapshot compaction, `@db.savepoint()`/`@db.rollback(point)` 메모리 상태 복원, 정적 파일 `@serve`, 그리고 일부 고급 도메인의 reference stub을 실행한다. CLI `orv db recover`는 raw WAL 또는 WAL archive manifest를 complete record count, unix ms timestamp, 또는 RFC3339 timestamp 경계까지 재생해 `@db.save` 호환 snapshot으로 복구하고, `orv db restore --wal/--archive --data --at`도 같은 replay path로 point-in-time snapshot을 복원한다. archive manifest 경로는 WAL hash/byte count를 검증한 뒤 사용한다. `orv db archive`는 WAL record/timestamp/hash manifest를 생성하며 `--target file://...`이면 WAL과 manifest를 file archive target으로 복사한다. `@server`는 tokio current-thread 런타임과 hyper HTTP/1.1 서버를 사용하며, boot body의 `@db.wal`/`@db.load`가 설정한 공유 DB handle을 route handler까지 유지하고, 매칭된 route의 origin id를 `x-orv-origin-id` 응답 헤더에 싣는다. 런타임은 attached server request frame을 공유 `orv.production.trace` JSON schema/file로 직렬화하는 helper도 소유하고, `ORV_RUNTIME_REQUEST_TRACE_PATH`가 있으면 graceful shutdown 때 같은 trace file을 쓴다.

### 4.5단계: Origin map artifact (orv-compiler)

```
HIR → origin map JSON
```

현재 `orv-compiler`는 HIR의 실행 가능한 도메인/라우트/응답/호출 노드에서 안정적인 origin id, source span fingerprint, traversal 기반 parent-child `contains` edge, call expression에서 resolved function으로 이어지는 `calls` edge를 생성한다. `orv origins <file>`은 이 artifact를 JSON으로 출력한다. `orv reveal <dir> <origin-id>`는 build artifact directory의 origin map, ProjectGraph, server runtime artifact, native server plan, native route source, bundle plan을 읽어 source span, graph node, route descriptor, native server target/build-run command/routes source summary 또는 client manifest/bundle target을 JSON으로 반환한다. `orv editor reveal <dir> <origin-id>`는 같은 origin id를 first-party editor focus/source/production navigation payload로 변환한다. `orv editor trace <dir> --trace <trace.json>`은 captured request trace frame의 origin id를 같은 editor navigation payload로 확장하고, `orv editor trace-stream <dir> --events <trace-events.sse>`는 EventSource body의 `orv:trace` snapshot과 `orv:trace.frame` payload를 같은 구조로 소비한다. DAP in-process attach와 env 기반 server run capture는 `orv-runtime`의 공유 `orv.production.trace` JSON schema/file writer를 사용하고, open-ended live streaming은 `/__orv/trace/events`의 유지 연결과 per-frame event로 시작됐다.

### 4.6단계: 초기 build artifact (orv-compiler + orv-cli)

```
HIR + ProjectGraph v1 → build-manifest.json + bundle-plan.json + origin-map.json + project-graph.json + source-bundle.json + server/app.orv-runtime.json + server/launch.json + server/native-server.json + server/runtime-image.json + server/native/Cargo.toml + server/native/main.rs + server/native/routes.rs | pages/index.html | client/manifest.json + client/reactive-plan.json + client/app.js + client/app.wasm
```

현재 `orv build <file-or-orv.toml> --out <dir>`은 native 프로덕션 바이너리를 만들지 않고 deterministic build artifact directory를 생성한다. 이 단계의 목적은 production bundler가 사용할 zero-overhead 입력 계약을 먼저 고정하는 것이다.

핵심 구조는 다음으로 제한한다.

- Graph artifacts: `build-manifest.json`, `bundle-plan.json`, `origin-map.json`, `project-graph.json`, `source-bundle.json`
- Server artifacts: `server/app.orv-runtime.json`, `server/launch.json`, `server/native-server.json`, `server/runtime-image.json`, `server/native/Cargo.toml`, `server/native/main.rs`, `server/native/routes.rs`
- Static/client artifacts: zero-runtime `pages/index.html`, 또는 interactive `client/manifest.json`/`client/reactive-plan.json`/`client/app.js`/`client/app.wasm`
- Verification/reveal surfaces: `orv verify-build`, `orv run-build`, `orv dev`, `orv reveal`, `orv lsp reveal`, `orv editor reveal`

`build-manifest.json`은 HIR origin map에서 추론한 `runtime_features`와 artifact inventory를 기록하고, `bundle-plan.json`은 future bundler target을 선언한다. `source-bundle.json`은 source path/source/content hash snapshot을 보존해 원본 파일 없이도 reveal/LSP reveal과 artifact reanalysis가 source span을 복구하게 한다. Server artifact는 route/listen/runtime/source-bundle 계약을 담고, native plan은 아직 final binary가 아닌 reference launcher package, route table/matcher source, runtime image plan, build/run command, `native-codegen`/`native-runtime-image` blocker를 기록한다. Client artifact는 manifest/reactive-plan/JS/WASM/source-bundle hash 계약으로 current source-bound initial render와 future dynamic/reactive codegen 입력을 분리한다.

운영 세부와 검증 항목은 [OPERATIONAL_SURFACES.md](OPERATIONAL_SURFACES.md)의 build/deploy 섹션에 둔다. 언어 목표 모델과 구현 상태 delta는 [SPEC.md](SPEC.md) §13에 둔다.

`server/runtime-image.json`은 reference runtime image, planned OCI image tag, native binary path, route/listen/runtime feature shape를 같은 schema로 고정한다. `server/native/routes.rs`는 같은 route descriptors를 Rust route table and matcher source로 고정해 future native router codegen 입력을 만든다. `deploy/manifest.json`은 이를 `server.native_runtime_image_plan`과 `server.native_routes_source`로 참조하고, `orv verify-build`는 native plan, image plan, route source가 같은 server artifact를 가리키는지 검증한다.

### 로드맵: 의미 기반 프로젝트 그래프 확장

```
HIR + ProjectGraph v1 → 의미 기반 프로젝트 그래프
```

라우트-페이지 연결, 데이터 의존성, 호출 그래프, 번들 포함 여부, DB schema 영향 범위처럼 의미 분석이 필요한 관계는 HIR 기반 확장 단계에서 추가한다.

### 로드맵: 최적화 및 코드 생성 (orv-compiler)

```
HIR + 프로젝트 그래프 → 최적화된 출력 코드
```

향후 프로젝트 특화 최적화를 수행한다:
- **DCE**: 모듈/도메인/기능 수준 데드 코드 제거
- **Auto-batching**: 루프 내 fetch를 단일 배치 요청으로 변환
- **Auto-parallelization**: 독립적 쿼리의 병렬 실행
- **렌더링 전략 추론**: 페이지별 SSG/CSR/SSR 자동 결정
- **번들 분할**: 서버/클라이언트 코드 자동 분리

### 로드맵: 번들 출력

```
최적화된 코드 → 실행 가능 번들
```

서버 네이티브 바이너리와 동적/최적화 클라이언트 WASM/JS 코드젠은 아직 로드맵이다. 현재는 서버 entry에서 `server/native-server.json` planned binary contract와 `server/native/Cargo.toml`/`server/native/main.rs` reference launcher package를, `let sig` 또는 client-side HTML await가 필요한 entry에서 page shell, client manifest URL, reactive plan, source bundle entry/hash를 담고 런타임 manifest/reactive-plan/source hash check를 수행하는 `ORV_CLIENT_BOOTSTRAP` metadata JS bootstrap, `orv.client` custom section, `orv_start`, initial-render memory, `orv_render_ptr`, `orv_render_len` exports를 담은 유효 WASM module인 `client/app.wasm`, 그리고 page/reactive-plan/loader/WASM/source hash/export/initial-render 계약을 묶는 `client/manifest.json`을 출력해 bundle/verify/deploy/reveal 계약을 먼저 고정한다.

## 로드맵 번들 출력 구조

```
dist/
├── server
│   ├── app              # 서버 네이티브 바이너리 (Rust 컴파일)
│   └── launch.json      # 현재 MVP reference runner launch 계약
├── client/
│   ├── app.wasm          # 클라이언트 WASM (sig 사용 페이지만)
│   ├── app.js            # WASM 바인딩 글루 코드
│   └── style.css         # @design에서 추출된 스타일
├── static/               # @serve로 지정된 정적 에셋
└── pages/
    └── *.html            # SSG 페이지 (정적 렌더링 결과)
```

### 번들 분할 전략

| 페이지 특성 | 출력 | JS/WASM 포함 |
|-------------|------|-------------|
| sig 없는 정적 페이지 | `pages/*.html` | 없음 (Zero-runtime) |
| sig 있는 대화형 페이지 | `pages/*.html` + `client/app.wasm` | WASM |
| 서버 라우트 | `server/app` | — |
| 혼합 (SSR + 대화형) | 서버 렌더링 + 부분 hydration | 해당 컴포넌트만 WASM |

## 보조 인프라

### 진단 (orv-diagnostics)

컴파일러의 모든 단계에서 발생하는 에러, 경고, 힌트를 `codespan-reporting` 기반으로 포매팅한다. `Span` 정보를 활용해 소스 코드의 정확한 위치를 밑줄과 함께 표시한다.

### Proc-macro (orv-macros)

컴파일러 내부에서 반복적인 보일러플레이트를 줄이기 위한 derive 매크로를 제공한다. `syn`/`quote` 기반.

### CLI (orv-cli)

`clap` 기반 CLI로 다음 커맨드를 제공한다:
- `orv init <dir> --name <name> [--template basic|shop]` — 최소 프로젝트 또는 쇼핑몰 `GET /` HTML form 홈, route scaffold, 검증/Compose 배포 README 생성
- `orv run <file>` — 파일을 로드/검사한 뒤 레퍼런스 런타임으로 실행
- `orv check <file>` — 파싱, 이름 해석, 타입/도메인 진단만 수행
- `orv dump <file>` — AST 디버그 출력
- `orv origins <file>` — HIR 기반 origin map JSON 출력
- `orv graph <file> [--view --out <dir>]` — AST ProjectGraph v1 + HIR origin map/edge JSON 출력 또는 정적 ProjectGraph HTML view artifact 생성
- `orv test <path> --filter <name> --list` — `.orv` 파일을 찾아 `test "name"` 블록이 있는 파일을 reference runtime 으로 실행하거나 발견 목록 JSON 출력
- `orv editor snapshot/reveal/runtime/debug/run-debug/export/trace/trace-stream` — first-party editor bootstrap JSON, source-hash watch set, build-origin navigation payload, runtime inspection pane JSON, DAP control transport/result JSON, static editor shell artifact with ProjectGraph/panel-list/runtime-frame/trace-detail rendering and optional trace state, captured request trace navigation payload와 EventSource snapshot/frame consumption payload 출력
- `orv build <file-or-orv.toml> --out <dir> [--prod]` — 초기 build manifest + bundle plan + origin map + project graph + server runtime/launch artifact, HTML-only static page, 또는 client manifest/reactive-plan/page/JS/WASM bootstrap 출력, prod profile이면 deploy manifest/container/Dockerfile/Compose/runbook/entrypoint 추가
- `orv add/remove` — `orv.toml` dependency section 편집과 lockfile 재생성
- `orv lock [dir-or-orv.toml] [--check]` — `orv.toml` project/dependency metadata와 local/file/HTTP/HTTPS registry `index.json` 기반 prerelease/build-aware exact/wildcard/caret/tilde/comparator/disjunction range 결과, optional `auth_token_env` 이름을 deterministic `orv.lock` JSON으로 고정하거나 최신성 검증
- `orv fetch [dir-or-orv.toml] --out <dir>` — 최신 lockfile에서 path/local registry 및 optional Bearer auth HTTP/HTTPS registry source-bundle dependency cache와 `deps-manifest.json` 출력
- `orv workspace new <member> [--root <dir>] [--name <name>]` — workspace root manifest member 등록과 member project scaffold 생성
- `orv workspace graph [root] [--view] [--out <dir>]` — workspace member graph/files/dependencies와 member 간 path dependency edge artifact 또는 member/edge search filter가 있는 정적 workspace graph HTML view 출력
- `orv workspace lock [root] --out <dir>` — dependency-first member lockfile artifact들과 `workspace-lock.json` 출력
- `orv workspace fetch [root] --out <dir>` — dependency-first member dependency cache artifacts와 `workspace-fetch.json` 출력
- `orv workspace build [root] --out <dir> [--prod] [--incremental]` — path dependency edge 기반 dependency-first 순서로 각 workspace member를 기존 build pipeline으로 빌드/검증하고 unchanged source hash member는 skip 하는 top-level workspace build manifest 출력
- `orv verify-build <dir>` — build manifest/plan target 존재, source bundle hash, deploy container/runtime image/Compose/runbook contract, server artifact, static page zero-runtime shape, client manifest/reactive-plan/page/JS/WASM bootstrap, optional dev HMR/watch/transport/event/server manifest 검증
- `orv verify-artifact <file>` — server runtime artifact source hash/route descriptor 검증
- `orv check-artifact <file>` — server runtime artifact source bundle 재분석
- `orv check-build <dir>` — build-level source bundle 재분석
- `orv run-artifact <file>` — server runtime artifact source bundle 재수화 + reference runtime 실행
- `orv run-build <dir>` — bundle plan 기준 reference server artifact 실행, 또는 static page HTML 출력
- `orv dev <file-or-orv.toml> --out <dir> [--hmr] [--watch] [--watch-loop] [--serve]` — build + verify-build + run-build reference dev bootstrap, optional HMR/watch transport/session manifest, opt-in poll loop event manifest, opt-in HTTP/1 EventSource endpoint manifest

로드맵 커맨드:
- native 서버 바이너리 + 동적/최적화 클라이언트 WASM/JS 코드젠/글루 번들 빌드
- 이름별 단일 test case 실행/async test isolation — 전체 test runner 확장

## Lint 정책

```toml
[workspace.lints.rust]
unsafe_code = "forbid"       # unsafe 코드 전면 금지
unused_must_use = "deny"

[workspace.lints.clippy]
all = "deny"                 # 모든 clippy 경고를 에러로
pedantic = "warn"            # 엄격한 스타일 검사
nursery = "warn"             # 실험적 린트 활성화
```
