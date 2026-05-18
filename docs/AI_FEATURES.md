# orv AI Features

이 문서는 orv first-party editor의 AI 기능 방향을 정리한다. 현재 구현/계약 상태는 [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md)를 기준으로 하고, 이 문서는 제품/학습 전략과 로드맵 성격을 가진다.

## 목표

AI autocomplete는 범용 코딩 보조가 아니라 orv DSL에 특화된 편집 보조여야 한다. 사용자가 `.orv` 파일에서 route, schema, domain, `@html`, `@db`, commerce/security scaffold를 작성할 때 문법적으로 유효하고, 프로젝트 convention에 맞으며, 실제 `orv check`/runtime 검증을 통과할 확률이 높은 제안을 주는 것이 목표다.

모델 단독으로 orv를 이해하게 만드는 방식보다, 에디터/컴파일러가 DSL 제약을 알고 모델은 좁혀진 후보를 자연스럽게 쓰는 구조를 우선한다.

```text
Editor buffer/context
 -> ORV parser/AST/ProjectGraph
 -> cursor position allowed completions
 -> spec/examples/project RAG context
 -> model completion
 -> parser/semantic/runtime validation
 -> ranked suggestions
```

## 기본 전략

초기에는 상용 LLM 또는 강한 추론 모델을 직접 autocomplete 모델로 믿지 않는다. 대신 다음 기반을 먼저 만든다.

- orv parser/grammar 기반 cursor position 분석
- 현재 파일 AST, ProjectGraph, import graph, 주변 source context
- `docs/SPEC.md`, scaffold, fixtures, best-practice examples 기반 RAG
- 가능한 token/block/domain 후보 제한
- completion 결과의 parser/formatter/semantic validator 통과 여부 검사
- 실패 completion의 진단과 수정 prompt loop

이 단계에서 상용 모델은 "모든 것을 아는 모델"이 아니라 constrained generator로 사용한다.

## 로컬 파인튜닝 모델 판단

로컬 파인튜닝 모델은 장기적으로 유효하다. 특히 autocomplete는 낮은 latency, 사내 코드 보호, 비용 예측성, 오프라인 사용성의 가치가 크다.

단, 처음부터 로컬 파인튜닝 모델을 제품 기반으로 삼는 것은 위험하다.

- orv DSL 데이터가 아직 작다.
- 좋은 completion이 무엇인지 평가셋이 없다.
- 문법 지식은 파인튜닝보다 parser/grammar/RAG가 더 정확하다.
- 모델 운영, serving, quantization, latency 튜닝 비용이 크다.

따라서 파인튜닝의 1차 목적은 "문법 암기"가 아니라 orv idiom과 팀 스타일을 맞추는 것이다. 문법 유효성은 parser와 semantic validator가 책임진다.

## 데이터 생성

강한 추론 모델로 synthetic data를 만드는 것은 필요하다. 0에서 1을 만들기 위한 초기 데이터 부트스트랩으로 쓴다. 단, 생성량보다 검증 파이프라인이 더 중요하다.

생성 대상:

- 완성된 좋은 `.orv` 파일
- partial prefix -> expected completion
- fill-in-the-middle completion
- 잘못된 `.orv` -> 수정본
- 자연어 요구사항 -> orv DSL
- 기존 orv 코드 -> 리팩터링/최적화
- anti-pattern -> best practice
- edge case와 recovery case

모든 synthetic sample은 최소한 다음 gate를 통과해야 한다.

- parser 통과
- formatter 통과
- semantic validator 통과
- 가능하면 `orv check`, 관련 runtime/test/smoke 통과
- 중복 제거
- 일부 human review

## 평가셋

파인튜닝 전에는 고정된 eval이 먼저 필요하다. eval 없이 학습하면 좋아졌는지 알 수 없다.

핵심 지표:

- syntax valid rate
- semantic valid rate
- `orv check` pass rate
- top-k completion accept rate
- edit distance to golden
- hallucinated keyword/domain rate
- project convention match rate
- latency p50/p95
- human acceptance rate

평가셋은 `docs/SPEC.md`, `fixtures/e2e`, `fixtures/plan`, shop scaffold, 실제 편집 로그에서 분리해 만든다. 학습 데이터와 평가 데이터는 반드시 분리한다.

## 학습 방향

초기 학습은 작은 코드 모델의 LoRA/QLoRA부터 시작한다. IDE autocomplete는 latency가 제품 품질이므로, 너무 큰 모델보다 1.5B~7B급 코드 모델을 우선 검토한다.

후보 계열:

- Qwen Coder 계열
- DeepSeek Coder 계열
- StarCoder 계열
- CodeLlama 계열

중요한 포맷은 fill-in-the-middle이다.

```text
<fim_prefix>
@route POST /checkout {
  let order = await @db.create Order {
    memberId: @session.id,
<fim_suffix>
  }
  @respond 201 order
}
<fim_middle>
    status: "pending",
    createdAt: @time.now()
```

학습 데이터는 "좋은 코드 전체"보다 editor autocomplete 상황에 가까운 prefix/suffix/cursor task 비중을 높인다.

## 단계별 로드맵

### Phase 1: Constrained Commercial Model

- parser/AST 기반 cursor context 생성
- allowed completion 후보 계산
- spec/examples RAG
- 상용 모델 completion
- parser/semantic validator로 결과 rerank

### Phase 2: Synthetic Dataset

- 강한 추론 모델로 best-practice `.orv` sample 생성
- partial/FIM/fix/refactor/task-to-DSL dataset 생성
- validator/test gate 자동화
- human-reviewed seed set 확보

### Phase 3: Eval Harness

- syntax/semantic/runtime/acceptance/latency 지표 고정
- golden task set과 regression set 분리
- 모델, prompt, retrieval, validator 변경을 같은 기준으로 비교

### Phase 4: Local Fine-Tuned Autocomplete

- 작은 코드 모델 LoRA/QLoRA 학습
- local serving/quantization/latency 측정
- parser-constrained rerank와 결합
- 실제 editor accept/reject 로그로 주기적 재학습

## 제품 원칙

- AI 기능은 MVP의 "AI 없이 5시간 쇼핑몰" 목표를 대체하지 않는다.
- AI는 초보자가 DSL을 더 빨리 쓰게 돕는 가속 장치다.
- 모델 제안은 항상 orv compiler/parser/validator의 제약 아래 둔다.
- 로컬 모델은 품질과 eval이 상용 모델+RAG baseline을 이길 때 editor 기본값 후보가 된다.
