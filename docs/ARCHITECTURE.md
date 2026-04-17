# orv 아키텍처

## 개요

orv 컴파일러는 Rust workspace로 구성된 10개 크레이트의 파이프라인 아키텍처를 따른다. 소스 코드는 단계별로 변환되어 최종적으로 서버 바이너리와 클라이언트 WASM/JS 번들로 출력된다.

소스 위치 타입(`Span`, `ByteRange`)은 별도 크레이트 대신 `orv-diagnostics`에 통합되어 진단 메시지와 함께 관리된다.

## 컴파일 파이프라인

```
  .orv 소스
      │
      ▼
┌────────────┐     ┌──────────────────┐
│ orv-syntax │────▶│  AST (구문 트리)  │
│ 렉서/파서   │     └──────────────────┘
└────────────┘
      │
      ▼
┌─────────────┐     ┌──────────────────────┐
│ orv-resolve │────▶│ 이름 해석된 AST       │
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
│ orv-project  │────▶│ 프로젝트 그래프           │
│ 그래프 추출   │     │ (도메인 관계, 의존성 맵)  │
└──────────────┘     └──────────────────────────┘
      │
      ▼
┌──────────────┐     ┌───────────────────────┐
│ orv-compiler │────▶│ 최적화된 출력 코드      │
│ 코드 생성     │     │ (DCE, batching, 분할)  │
└──────────────┘     └───────────────────────┘
      │
      ▼
┌──────────────┐     ┌──────────────────────────┐
│ orv-runtime  │────▶│ 실행 가능 번들           │
│ 어댑터 빌드   │     │ (서버 바이너리 + WASM)   │
└──────────────┘     └──────────────────────────┘
      │
      ▼
┌──────────┐
│ orv-cli  │  사용자 인터페이스 (orv 바이너리)
└──────────┘
```

진단/위치 정보(`orv-diagnostics`)는 파이프라인의 모든 단계가 공통으로 사용한다. `orv-core`/`orv-macros`는 공유 인프라로 별도 단계 없이 여러 크레이트가 참조한다.

## 크레이트 상세

| 크레이트 | 역할 | 주요 의존성 |
|----------|------|------------|
| `orv-diagnostics` | 소스 위치(`Span`, `ByteRange`) + 구조화된 컴파일러 진단 메시지. codespan-reporting 기반 포매팅 | codespan-reporting |
| `orv-macros` | Rust proc-macro 유틸리티. 컴파일러 내부에서 사용하는 derive 매크로 | syn, quote, proc-macro2 |
| `orv-core` | 핵심 타입 정의와 공유 인프라. 모든 크레이트가 공통으로 사용하는 타입 | orv-macros, orv-diagnostics, wgpu |
| `orv-syntax` | 렉서(Lexer)와 파서(Parser). `.orv` 소스를 AST로 변환 | orv-diagnostics |
| `orv-resolve` | 이름 해석(Name Resolution)과 스코프 분석. AST의 식별자를 선언에 연결 | orv-diagnostics, orv-syntax |
| `orv-hir` | 고수준 중간 표현(HIR) 정의. 의미 분석 이후의 타입 정보가 포함된 IR | — |
| `orv-analyzer` | 의미 분석(Semantic Analysis)과 HIR 로우어링. 타입 검사, 도메인 검증 | orv-diagnostics, orv-hir, orv-resolve, orv-syntax |
| `orv-project` | 프로젝트 그래프 추출. 멀티파일 프로젝트의 도메인 관계와 의존성 분석 | orv-hir, serde |
| `orv-compiler` | 프론트엔드 컴파일 파이프라인 통합. 최적화(DCE, batching, 병렬화)와 코드 생성 | orv-analyzer, orv-core, orv-diagnostics, orv-project, orv-syntax, orv-hir, orv-runtime |
| `orv-runtime` | 레퍼런스 런타임과 어댑터 빌드. 서버 바이너리 및 클라이언트 WASM 출력 | orv-hir, serde, serde_json, thiserror |
| `orv-cli` | CLI 프론트엔드. `orv` 바이너리로 컴파일되며 전체 파이프라인을 오케스트레이션 | orv-core, orv-diagnostics, orv-syntax, orv-analyzer, orv-compiler, orv-runtime, clap |

## 의존성 그래프

```
orv-diagnostics ◄───────────────────────────────┐
   │                                            │
   ├──────────┐                                 │
   ▼          ▼                                 │
orv-syntax  orv-macros                          │
   │          │                                 │
   │          ▼                                 │
   │       orv-core ◄───────────────────┐       │
   │                                    │       │
   ▼                                    │       │
orv-resolve                             │       │
   │                                    │       │
   ▼                  orv-hir ◄─────┐   │       │
orv-analyzer ─────────────┘         │   │       │
   │                                │   │       │
   │                          orv-project       │
   │                                │   │       │
   │                          orv-runtime       │
   │                                │   │       │
   ▼                                ▼   │       │
orv-compiler ◄──────────────────────┘───┘───────┘
   │
   ▼
orv-cli
```

## 데이터 흐름

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

### 4단계: 프로젝트 분석 (orv-project)

```
HIR → 프로젝트 그래프
```

멀티파일 프로젝트에서 도메인 간 관계, 라우트-페이지 연결, 데이터 의존성을 그래프로 추출한다.

### 5단계: 최적화 및 코드 생성 (orv-compiler)

```
HIR + 프로젝트 그래프 → 최적화된 출력 코드
```

프로젝트 특화 최적화를 수행한다:
- **DCE**: 모듈/도메인/기능 수준 데드 코드 제거
- **Auto-batching**: 루프 내 fetch를 단일 배치 요청으로 변환
- **Auto-parallelization**: 독립적 쿼리의 병렬 실행
- **렌더링 전략 추론**: 페이지별 SSG/CSR/SSR 자동 결정
- **번들 분할**: 서버/클라이언트 코드 자동 분리

### 6단계: 번들 출력 (orv-runtime)

```
최적화된 코드 → 실행 가능 번들
```

서버 바이너리와 클라이언트 WASM을 생성한다.

## 번들 출력 구조

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
- `orv build` — 프로젝트 빌드
- `orv check` — 타입 검사만 수행
- `orv dev` — 개발 서버 실행

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
