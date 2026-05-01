# orv 아키텍처

## 개요

orv는 Rust workspace로 구성된 10개 크레이트의 파이프라인 아키텍처를 따른다. 현재 구현은 `.orv` 소스를 로드/파싱/해석/분석한 뒤 HIR을 레퍼런스 tree-walking 런타임으로 실행하는 MVP다. `orv-compiler`는 HIR 기반 origin map artifact를 생성할 수 있고, `@server` 런타임은 매칭된 route origin id를 HTTP 응답 헤더로 노출한다. 서버 바이너리와 클라이언트 WASM/JS 번들 출력은 아직 구현되지 않은 컴파일러 로드맵이다.

이 문서는 **현재 구현 구조와 데이터 흐름**을 설명하는 문서다. 언어 문법과 의미론의 공식 기준은 `docs/SPEC.md`이며, 이 문서는 그 사양을 구현 관점에서 해설한다.

소스 위치 타입(`Span`, `ByteRange`)은 별도 크레이트 대신 `orv-diagnostics`에 통합되어 진단 메시지와 함께 관리된다.

## 컴파일 파이프라인

```
  orv-cli (`run` / `check` / `dump` / `origins`)
      │
      ▼
┌──────────────┐
│ orv-project  │  파일/Import 로드
│ + orv-syntax │  각 파일 lex/parse
└──────┬───────┘
       │ 병합된 AST Program
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

진단/위치 정보(`orv-diagnostics`)는 파이프라인의 모든 단계가 공통으로 사용한다. `orv-core`/`orv-macros`는 공유 인프라로 별도 단계 없이 여러 크레이트가 참조한다. `orv-compiler`는 현재 origin map artifact 생성을 담당하며, 향후 최적화/번들링 단계가 들어갈 자리다.

## 크레이트 상세

| 크레이트 | 역할 | 주요 의존성 |
|----------|------|------------|
| `orv-diagnostics` | 소스 위치(`Span`, `ByteRange`) + 구조화된 컴파일러 진단 메시지. codespan-reporting 기반 포매팅 | codespan-reporting |
| `orv-macros` | Rust proc-macro 유틸리티. 컴파일러 내부에서 사용하는 derive 매크로 | syn, quote, proc-macro2 |
| `orv-core` | 핵심 타입 정의와 공유 인프라. 모든 크레이트가 공통으로 사용하는 타입 | orv-macros, orv-diagnostics, wgpu |
| `orv-syntax` | 렉서(Lexer)와 파서(Parser). `.orv` 소스를 AST로 변환 | orv-diagnostics |
| `orv-resolve` | 이름 해석(Name Resolution)과 스코프 분석. AST의 식별자를 선언에 연결 | orv-diagnostics, orv-syntax |
| `orv-hir` | 고수준 중간 표현(HIR) 정의. 의미 분석 이후의 타입 정보와 origin id 계산 규칙이 포함된 IR | — |
| `orv-analyzer` | 의미 분석(Semantic Analysis)과 HIR 로우어링. 타입 검사, 도메인 검증 | orv-diagnostics, orv-hir, orv-resolve, orv-syntax |
| `orv-project` | entry 파일에서 import를 따라 멀티파일 프로그램을 로드/병합. 프로젝트 그래프 추출은 로드맵 | orv-syntax, orv-diagnostics, thiserror, serde |
| `orv-compiler` | HIR origin map artifact 생성. 코드 생성/최적화 단계는 로드맵 | orv-analyzer, orv-core, orv-diagnostics, orv-project, orv-syntax, orv-hir, orv-runtime |
| `orv-runtime` | 레퍼런스 tree-walking 런타임. `@server`는 hyper HTTP/1.1 서버로 실행하며 route 응답에 `x-orv-origin-id`를 붙인다 | orv-diagnostics, orv-hir, orv-syntax, serde, serde_json, regex, thiserror, tokio, hyper |
| `orv-cli` | CLI 프론트엔드. 현재 `run`, `check`, `dump`, `origins`로 로드/해석/분석/실행과 origin map 출력을 오케스트레이션 | orv-core, orv-diagnostics, orv-syntax, orv-resolve, orv-analyzer, orv-compiler, orv-project, orv-runtime, clap |

## 의존성 그래프

```
orv-cli
├── orv-project ──▶ orv-syntax ──▶ orv-diagnostics
├── orv-resolve ──▶ orv-syntax ──▶ orv-diagnostics
├── orv-analyzer ─▶ orv-hir / orv-resolve / orv-syntax / orv-diagnostics
├── orv-runtime ──▶ orv-hir / orv-syntax / orv-diagnostics
└── orv-compiler ─▶ orv-hir / orv-analyzer / orv-project / orv-runtime

orv-core ──▶ orv-macros / orv-diagnostics
```

## 데이터 흐름

### 0단계: 프로젝트 로드 (orv-project)

```
entry .orv → import DFS → 병합된 AST Program
```

entry 파일에서 시작해 `import` 문을 따라 `.orv` 파일을 재귀적으로 로드한다. 현재는 import 된 파일의 top-level 문장을 entry 앞에 붙여 하나의 AST `Program`으로 병합한다. 파일별 scope 격리, visibility enforcement, 외부 레지스트리 의존성, 정교한 사이클 진단은 로드맵이다.

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

현재 런타임은 HIR을 직접 평가한다. 일반 표현식, 함수, 타입/캐스트, HTML 값, 서버 라우트, 인메모리 `@db`, 정적 파일 `@serve`, 그리고 일부 고급 도메인의 reference stub을 실행한다. `@server`는 tokio current-thread 런타임과 hyper HTTP/1.1 서버를 사용하며, 매칭된 route의 origin id를 `x-orv-origin-id` 응답 헤더에 싣는다.

### 4.5단계: Origin map artifact (orv-compiler)

```
HIR → origin map JSON
```

현재 `orv-compiler`는 HIR의 실행 가능한 도메인/라우트/응답/호출 노드에서 안정적인 origin id와 source span fingerprint를 생성한다. `orv origins <file>`은 이 artifact를 JSON으로 출력한다. 프로덕션 trace와 editor reveal을 origin map에 연결하는 단계는 로드맵이다.

### 로드맵: 프로젝트 그래프 분석

```
HIR → 프로젝트 그래프
```

멀티파일 프로젝트에서 도메인 간 관계, 라우트-페이지 연결, 데이터 의존성을 그래프로 추출한다.

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

서버 바이너리와 클라이언트 WASM 생성은 아직 구현되어 있지 않다.

## 로드맵 번들 출력 구조

```
dist/
├── server
│   └── app              # 서버 네이티브 바이너리 (Rust 컴파일)
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
- `orv run <file>` — 파일을 로드/검사한 뒤 레퍼런스 런타임으로 실행
- `orv check <file>` — 파싱, 이름 해석, 타입/도메인 진단만 수행
- `orv dump <file>` — AST 디버그 출력
- `orv origins <file>` — HIR 기반 origin map JSON 출력

로드맵 커맨드:
- `orv build` — 프로젝트 빌드
- `orv dev` — 개발 서버 실행
- `orv test` — orv 테스트 실행

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
