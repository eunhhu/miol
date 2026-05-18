# orv 문서 구조 가이드

## 목적

이 문서는 orv 문서 체계의 역할 분리와 수정 원칙을 정리한다. 같은 주장을 여러 곳에서 반복하기보다, 각 문서가 담당하는 질문을 명확히 나눈다.

## 1. 비전 문서

대상 파일:

- `docs/README.md`

다루는 내용:

- 왜 이 언어를 만드는가
- 어떤 사용자와 생산성 목표를 상정하는가
- 어떤 방향성과 철학을 우선하는가
- 문서 전체를 어떻게 읽으면 되는가

다루지 않는 내용:

- 세부 문법의 authoritative 정의
- 구현 세부 동작의 최종 판정

## 2. 기준 사양

대상 파일:

- `docs/SPEC.md`

다루는 내용:

- 현재 기준의 공식 문법
- 의미론과 타입 규칙
- 도메인별 동작 정의
- 컴파일타임 규칙과 공식 예시

다루지 않는 내용:

- 구현 상태 표
- 날짜형 changelog
- CLI/LSP/DAP/build/DB 운영 surface의 전체 목록

판정 원칙:

- 문서 간 충돌이 있으면 언어 의미론은 `docs/SPEC.md`를 우선 기준으로 해석한다.
- 예제 파일이 `SPEC.md`와 다르면, 먼저 `SPEC.md`를 맞다고 본다.
- 예제가 더 나은 방향을 보여준다면, 그것은 사양 변경 후보이지 현재 사양 자체는 아니다.
- 구현/계약 상태 판단은 `docs/IMPLEMENTATION_MATRIX.md`를 기준으로 한다.

## 2.5 MVP / 상태 / 운영 문서

대상 파일:

- `docs/MVP.md`
- `docs/IMPLEMENTATION_MATRIX.md`
- `docs/IMPLEMENTATION_STATUS.md`
- `docs/IMPLEMENTATION_GAP_REPORT.md`
- `docs/OPERATIONAL_SURFACES.md`
- `docs/AI_FEATURES.md`
- `docs/ROADMAP.md`
- `docs/CHANGELOG.md`

다루는 내용:

- `MVP.md`: 지금 되는 것, MVP 포함/제외 범위
- `IMPLEMENTATION_MATRIX.md`: 상태, 계약 레벨, milestone, crate, fixture, CLI 표
- `IMPLEMENTATION_STATUS.md`: 상태 용어와 빠른 요약
- `IMPLEMENTATION_GAP_REPORT.md`: 전체 문서 대비 진행률, 남은 기능, 리스크 분석 보고서
- `OPERATIONAL_SURFACES.md`: CLI/LSP/DAP/build/DB 같은 운영 surface 세부
- `AI_FEATURES.md`: first-party editor AI autocomplete, RAG, 평가셋, synthetic data, 로컬 파인튜닝 전략
- `ROADMAP.md`: 미래 기능
- `CHANGELOG.md`: 날짜가 붙은 구현 델타

판정 원칙:

- 구현/계약 상태는 `IMPLEMENTATION_MATRIX.md`가 기준이다.
- `IMPLEMENTATION_GAP_REPORT.md`는 상태표의 파생 분석이다. 진행률/리스크/우선순위를 요약하되, 기능별 authoritative 판정은 `IMPLEMENTATION_MATRIX.md`에 남긴다.
- 운영 command/method 세부는 `OPERATIONAL_SURFACES.md`가 기준이다.
- 미래 기능은 `ROADMAP.md`에만 둔다.
- 에디터 AI 제품/학습 전략은 `AI_FEATURES.md`에 둔다.
- 날짜형 보충은 `CHANGELOG.md`로 보낸다.

## 3. 실험 사양 / 탐색 예제

대상 파일:

- `fixtures/default-syntax.orv`
- `fixtures/plan/*.orv`

다루는 내용:

- 사용감 탐색
- 문법 압박 테스트
- 미래 방향 실험
- 설명용 대형 예제

해석 원칙:

- 이 파일들은 설계 의도를 드러내는 중요한 자료이지만, 기본적으로는 탐색 공간이다.
- `SPEC.md`와 완전히 일치하지 않을 수 있다.
- 일치하지 않는 부분은 버그일 수도 있고, 아직 확정되지 않은 아이디어일 수도 있다.
- 따라서 예제를 수정할 때는 항상 `SPEC.md` 기준과 함께 읽는다.

## 4. 구현 아키텍처 문서

대상 파일:

- `docs/ARCHITECTURE.md`

다루는 내용:

- 현재 Rust workspace 구조
- 크레이트 책임 분리
- 데이터 흐름과 파이프라인
- 구현 관점의 제약

다루지 않는 내용:

- 언어 의미론의 최종 판정
- 표면 문법의 공식 정의

## 5. 실행 검증 예제

대상 파일:

- `fixtures/e2e/*.orv`

다루는 내용:

- 핵심 라우팅/미들웨어/경로 처리 검증
- 실제 동작 회귀 방지에 가까운 작은 예제

## 수정 규칙

- 비전과 방향을 바꾸면 `docs/README.md`를 수정한다.
- MVP 경계가 바뀌면 `docs/MVP.md`를 수정한다.
- 구현 상태나 계약 레벨이 바뀌면 `docs/IMPLEMENTATION_MATRIX.md`를 먼저 수정한다.
- 날짜형 구현 보충은 `docs/CHANGELOG.md`에 추가한다.
- 공식 문법이나 의미론을 바꾸면 `docs/SPEC.md`를 수정한다.
- 사용감이나 미래 방향을 실험하면 `fixtures/default-syntax.orv` 또는 `fixtures/plan/*.orv`를 수정한다.
- 구현 구조가 바뀌면 `docs/ARCHITECTURE.md`를 수정한다.
- 에디터 AI autocomplete, synthetic data, eval, fine-tuning 방향을 바꾸면 `docs/AI_FEATURES.md`를 수정한다.
- 문서 간 충돌을 발견하면, 우선 `SPEC.md`와 예제의 차이를 명시적으로 판단한다.

## 권장 읽기 순서

1. `docs/README.md`
2. `docs/MVP.md`
3. `docs/IMPLEMENTATION_MATRIX.md`
4. `docs/IMPLEMENTATION_GAP_REPORT.md`
5. `docs/SPEC.md`
6. `docs/ARCHITECTURE.md`
7. `docs/OPERATIONAL_SURFACES.md`
8. `docs/AI_FEATURES.md`
9. `docs/IMPLEMENTATION_STATUS.md`
10. `fixtures/default-syntax.orv`
11. `fixtures/plan/*.orv`
12. `fixtures/e2e/*.orv`
