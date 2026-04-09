---
status: paused
type: brainstorm-notes
topic: orv-db (type-native embedded database)
date: 2026-04-08
blocked_by:
  - "ORV 표준 타입 체계 확정 (원시 / Vec / HashMap / Object / Function / Domain / File)"
  - "orv-storage 스토리지 백엔드 추상화 확정 (File 타입과 object store 모델)"
resume_after:
  - "위 두 스펙이 완료된 뒤"
---

# orv-db Brainstorm — Paused Notes

이 문서는 `orv-db` (ORV 전용 임베디드 타입-네이티브 데이터베이스) 브레인스토밍 도중 **표준 타입 체계와 스토리지 추상화가 먼저 정리돼야 한다는 판단이 내려져 일시 중단된 시점**의 맥락을 보존하는 메모다. 나중에 재개할 때 처음부터 다시 파지 말고 이 문서부터 읽고 들어올 것.

## 왜 멈췄는가

1. 사용자가 "스토리지 시스템을 처음부터 1급으로 설계한다"는 요구를 추가했다. 기존 DB들(SQLite, Postgres, RocksDB)은 로컬 파일을 전제로 엔진을 짠 뒤 S3/GCS 호환을 "확장"으로 붙였는데, `orv-db`는 초기 설계부터 스토리지 백엔드를 1급 시민으로 가정한다.
2. 동시에 ORV의 표준 타입 체계 (원시 / `Vec` / `HashMap` / `Object` (JSON literal) / `Function` / `Domain` (`@xxx`) / `File`) 가 아직 완전히 정비되지 않았다.
3. `orv-db`의 전제는 **"ORV struct/enum이 곧 DB 레코드"** 이므로, 표준 타입이 흔들리면 레코드 레이아웃·코덱·쿼리 표현식 전부 재작업된다.
4. 특히 `File` 타입은 단순한 "파일"이 아니라 **DB와 object store의 경계를 허무는 1급 시민**으로 보인다. 레코드 필드로 `File`이 들어가면 그것은 blob을 레코드에 박는 게 아니라 "스토리지 백엔드의 오브젝트 참조"를 박는 것이다. 이 의미는 타입 체계 쪽에서 먼저 확정돼야 한다.

→ 결정: **타입 체계 → 스토리지 백엔드 → DB 엔진** 순서로 스펙을 분리해서 작성한다. 지금 시점에서는 DB 브레인스토밍을 중단하고, 앞선 두 스펙이 완료된 뒤 이 문서에서 맥락을 복원해 재개한다.

## 지금까지 합의된 것 (고정 요구사항)

### 정체성

- `orv-db`는 ORV 전용 임베디드·단일 파일(잠정)·타입 네이티브 데이터베이스다.
- 범용 DB가 아니며, 범용성을 버리는 대가로 **ORV 컴파일러가 아는 모든 정보를 전부 활용**해 런타임 오버헤드를 제거한다.
- 한 문장 정의: "ORV struct/enum이 디스크 바이트 레이아웃 그 자체가 되고, 쿼리는 ORV 표현식으로 컴파일 타임에 실행 코드로 생성되며, 읽기는 락 프리이고 쓰기는 copy-on-write로 원자적인, 단일 파일 임베디드 엔진."
- (보류 사항: 스토리지 1급 요구에 따라 "단일 파일" 가정은 재검토 대상으로 바뀜. 재개 시 가장 먼저 다시 판단할 지점.)

### 목표 축 (Q1~Q6 단계적으로 확정됨)

- **Q1 최적화 축**: A (스키마 고정으로 플래너 제거) + B (JSON/문서 유연성). **확정**.
- **Q2 스케일 프로파일**: "스케일링에 유연·자유로워야 하고 오버헤드 없이 대규모 프로젝트도 커버하거나 더 좋은 성능." → 임베디드 소규모부터 수십 GB까지, 오버헤드 없이.
- **Q3 워크로드**: **C + D** — 혼합 읽기/쓰기 + 모델 단위 자동/명시적 최적화 선택. `@append-only` / `@timeseries` 같은 애너테이션으로 특정 모델은 다른 스토리지 백엔드로 라우팅 가능.
- **Q4 트랜잭션**: **B + D** — Snapshot isolation 기본 + 모델 단위 옵트아웃. 단일 writer, 락 프리 읽기, MVCC는 페이지 CoW로 자연 달성. 다중 writer(SSI)는 MVP 비목표.
- **Q5 JSON/데이터 모델**: **핵심 방향 수정됨**. JSON 중심이 아니라 **"ORV의 struct/enum/Vec/HashMap 타입이 DB에 그대로 녹아들도록"**. 타입 시스템이 스키마 검증까지 수행하고, 그 결과가 그대로 레코드 레이아웃이 된다.
- **Q6 핵심 가치 축**: **A (struct 그대로 저장, ORM 0) + B (레이아웃 효율, 역직렬화 0) + D (쿼리 타입 안전성)**. 특히 A와 B가 최우선.

### MVP 최적화 목록 (Opt-1 ~ Opt-7) — 전부 MVP 범위 확정

- **Opt-1. 컴파일 타임 쿼리 코드 생성**: 런타임 플래너/옵티마이저/프리페어드 캐시 전부 0. ORV HIR lowering 패스에서 쿼리 표현식을 DB API 호출 시퀀스로 낮춤.
- **Opt-2. 레코드 역직렬화 0**: 디스크 바이트 = struct 바이트. Pod 필드는 `bytemuck::from_bytes` 수준의 safe 캐스팅으로 제로 카피. 가변 길이 필드는 tail + 오프셋 슬롯.
- **Opt-3. OS 페이지 캐시 직접 활용**: 자체 버퍼 풀 없음. mmap 기본. (스토리지 1급 요구에 따라 로컬 백엔드 한정으로 전락할 수 있음 — 재개 시 재판단.)
- **Opt-4. 컴파일 타임 인덱스 접근 함수 생성**: `@index email` → `users_by_email(tx, "...") -> Option<User>`가 빌드 타임에 존재. 동적 인덱스 메타 조회 없음.
- **Opt-5. 락 프리 스냅샷 isolation**: CoW 루트 포인터 atomic 로드 하나. MVCC 버전 체인, 락 매니저, GC 전부 없음.
- **Opt-6. 자동 배치 커밋**: 단일 writer 큐에서 대기 중인 트랜잭션을 자동 배칭, fsync 한 번에 여러 커밋 영속화.
- **Opt-7. 동일 프로세스 = IPC/네트워크/이중 직렬화 0**: 쿼리 결과 = ORV 서버 핸들러의 struct 그 자체.

### 고급 기능 (v2+ 유보)

- SIMD, io_uring, JIT, 벡터 인덱스, full-text search, 지리 인덱스, 압축, 온라인 스키마 변경, 다중 writer(SSI), 복제/샤딩.

### 성공 기준 (잠정)

- **기능**: `fixtures/project-e2e/src/lib/db.orv`의 `User / Post / Like` 스키마가 실제로 저장·조회·인덱싱되고 e2e 통과.
- **성능**: 포인트 조회 P99 < 10μs (메모리 히트), 인덱스 조회 P99 < 20μs, 단일 insert P99 < 100μs (배치), 동시 읽기 100k tps.
- **크기**: 단일 파일 배포. `orv-db` 외부 의존성 최소.
- **안전성**: `unsafe_code = forbid` 유지. Crash 후 마지막 성공 커밋까지 복구.

## 섹션 1/7 (정체성·목표·비목표) — 사용자 승인 완료

위 "지금까지 합의된 것"이 섹션 1의 내용이다. 사용자가 "OK" 로 승인했다.

## 섹션 2/7 (아키텍처 개요 & 크레이트 구조) — 사용자 승인 완료

### 레이어 다이어그램

```
L7: Query Codegen        — ORV 쿼리 AST → DB API 호출 (orv-compiler 확장)
L6: Schema Layout        — @model → TypeLayout (오프셋, 크기, 인덱스 디스크립터)
L5: Typed Table API      — Table<T>, Index<T, F>, cursor, iterator + 레코드 코덱
L4: Transaction          — ReadTx, WriteTx, 배치 커밋, 루트 atomic swap
L3: B+Tree Engine        — CoW B+tree, 페이지 split/merge, 키 코덱
L2: Page Manager         — 페이지 할당/해제, CoW, 오버플로, free list
L1: File / Mmap          — 파일 I/O, mmap, fsync, 헤더 페이지
```

(보류 사항: L1~L3은 스토리지 1급 요구에 따라 재설계 대상. LSM + object store 모델로 기울 가능성 매우 높음. 재개 시 우선 판단.)

### 크레이트 구성

- 새 크레이트 `crates/orv-db/` 추가 (워크스페이스에 편입).
- 내부는 모듈로 분할: `file.rs`, `page.rs`, `btree.rs`, `txn.rs`, `table.rs`, `codec.rs`, `schema.rs`, `error.rs`.
- L7 (Query Codegen) 은 `orv-db`에 두지 않고 `orv-compiler` 쪽에 lowering 패스로. `orv-db`는 컴파일러를 모르고 런타임 API만 제공.
- 사용자가 `orv-db`라는 이름에 명시 동의 ("orv-db로 ㄱㄱ").

### 의존성 후보

`memmap2`, `bytemuck`, `parking_lot`, `thiserror`, `crc32fast`. `unsafe_code = forbid` 는 유지 (이 크레이트들이 내부 unsafe를 캡슐화하므로 사용자 코드는 safe).

### ORV 컴파일러 접점

- **스키마 수집 패스**: 모든 `@model` → `SchemaSet`.
- **쿼리 lowering 패스**: `users.where(u => ...)` → `orv_db::Table::<User>::scan_by_index(...)` 호출로 변환.
- **레코드 codec 생성 패스**: 각 `@model`에 대해 `encode_*` / `decode_*` 함수 emit.
- 현재 ORV는 HIR 기반 인터프리터 런타임이므로, lowering은 "Rust 코드 생성"이 아니라 "HIR 재작성 패스"로 구현.

## 섹션 3/7 (디스크 포맷) — 작성 직후 사용자가 방향 수정 요청

섹션 3 본문은 CoW B+tree 단일 파일을 전제로 작성되었으나, 사용자가 "스토리지 1급 설계"를 요구해 **해당 전제 자체가 무효화됨**. 섹션 3 본문의 세부(페이지 크기, 헤더 레이아웃, 슬롯 디렉토리 등)는 재개 시 재작성 대상이므로 여기에는 남기지 않는다. 재개 시 참조가 필요하면 본 대화 로그를 복원해 섹션 3 초안을 확인할 것.

다만 **섹션 3에서 검증된 "제로 오버헤드" 달성 원리**는 엔진 형태와 무관하게 유지되므로 기록한다:

| 항목 | 전통 DB | ORV DB 목표 |
|---|---|---|
| Row header (xmin/xmax/flags) | 20~24 B | 0 B (MVCC는 스토리지 수준 CoW/immutable로 처리) |
| NULL bitmap | 1 bit/field | 0 B (Option은 타입 시스템에서 처리) |
| 필드 오프셋 테이블 | 런타임 보관 | 0 B (컴파일 타임 상수) |
| 필드 이름 저장 | BSON/스키마 참조 | 0 B (타입 시스템이 이름 앎) |
| 역직렬화 CPU | 필드별 루프 | Pod 필드 1회 캐스팅, tail만 복사 |

## 스토리지 1급 설계가 엔진 선택에 주는 영향 (재개 시 첫 판단 지점)

### 가장 약한 스토리지 (S3/GCS) 의 제약

- 객체 단위 읽기/쓰기만 가능 (random write 없음).
- Append 불가 — PUT은 오브젝트 전체 교체.
- Listing 가능 (prefix).
- 원자적 conditional put (E-Tag / If-Match).
- Latency 밀리초 단위.
- 요청당 과금.

### 함의

- **랜덤 R/W를 전제한 페이지 기반 엔진 (전통 B+tree) 은 object store에 부적합**. 리프 수정마다 루트까지의 경로 전체를 새로 써야 하는 CoW B+tree는 "오브젝트 단위 PUT"과 궁합 나쁨.
- **LSM 계열의 "immutable object + manifest" 모델이 자연스러움**. 데이터는 불변 세그먼트(SST)로만 쓰고, 변경은 manifest 오브젝트를 새로 써서 반영. Manifest PUT의 conditional put이 단일 writer snapshot isolation의 자연스러운 구현이 됨.
- **레퍼런스**: SlateDB (Rust, S3 기반 LSM, 2024), Turbopuffer, Neon Pageserver, tigerbeetle 등. 이들이 "object-store 네이티브 LSM"의 최신 선행 사례.
- **로컬 파일은 "로컬 디렉토리에 오브젝트를 저장하는 특수한 object store"** 로 격하됨. mmap/zero-copy는 로컬 캐시 히트 경로에서만 유지.
- **Opt-2 (역직렬화 0)** 는 여전히 유효. 오브젝트를 fetch 해서 메모리에 로드한 뒤 그 안에서 Pod 캐스팅.
- **Opt-6 (배치 커밋)** 는 오히려 더 중요해짐. 원격 PUT 레이턴시가 밀리초 단위라 배칭 없이는 쓰기 처리량이 급락.
- **Opt-3 (OS 페이지 캐시 직접 활용)** 는 로컬 백엔드 한정 의미로 축소. 원격 백엔드에선 애플리케이션 레벨 캐시 필요.
- **Q3 (혼합 워크로드)** 요구와 LSM의 쓰기 친화성이 오히려 더 잘 맞음.

### 재개 시 첫 결정

**엔진 = object-store 네이티브 LSM** (잠정 방향) vs **로컬 전용 CoW B+tree 유지 + 원격은 별도 어댑터**. 후자는 "두 엔진 유지 비용" 때문에 비추지만, 타입 체계와 스토리지 스펙이 끝난 뒤 최종 판단.

## 표준 타입 각각이 DB 설계에 던지는 질문 (타입 체계 스펙에서 답해야 할 항목)

| 타입 | DB/스토리지 관점 질문 |
|---|---|
| 원시 (i*/u*/f*/bool) | Pod 캐스팅 가능. 문제 없음. |
| `string` | UTF-8 length-prefixed? null-terminated? 가변 길이이므로 tail 대상. |
| `Vec<T>` (JS Array 역할) | 요소 크기 가변 가능? 랜덤 접근 보장? 가변 길이 tail. |
| `HashMap<K,V>` | 반복 결정성? 디스크에서 정렬된 키-값 쌍? `m.get(k)` O(1) 유지? |
| `Object` (JSON literal) | **가장 큰 질문**. 필드가 컴파일 타임에 고정인가 동적인가? 동적이면 제로 오버헤드 원칙과 충돌 → DB 안에선 blob으로 자동 직렬화 필요. |
| `Function` | Closure의 캡처 환경 때문에 직렬화 불가. DB 레코드 필드로는 불허가 원칙. 식별자만 저장 허용? |
| `Domain` (`@xxx`) | ORV만의 1급 시민. 정적 구조면 컴파일 타임 레이아웃 생성 가능, 동적이면 blob. 기존 DB에 전례 없음. |
| `File` | **스토리지 1급 요구의 핵심 교차점**. File 필드는 "스토리지 백엔드의 오브젝트 참조"여야 함 — 실제 바이트는 object store의 별도 키에, 레코드는 참조만. 삭제 시 GC 또는 refcount 필요. |

## 재개 체크리스트

재개 시 다음 순서로 이 문서를 다시 살린다:

1. 이 문서를 처음부터 끝까지 읽는다.
2. 스펙 A (표준 타입 체계) 및 스펙 B (`orv-storage` 백엔드 추상화) 결과를 확인한다.
3. "스토리지 1급 설계가 엔진 선택에 주는 영향" 절을 기준으로 **엔진 형태를 재판단** (object-store 네이티브 LSM 유력).
4. 재판단한 엔진으로 섹션 2 (아키텍처) 를 업데이트 — 특히 L1~L3 레이어 의미 변경.
5. 섹션 3 (디스크 포맷) 을 새로 작성. 단위가 "페이지"에서 "오브젝트"로 바뀔 가능성 높음.
6. 섹션 4~7 (트랜잭션, 코덱, 쿼리 lowering, 에러/테스트) 을 이어서 작성.
7. 최종 스펙을 `docs/specs/YYYY-MM-DD-orvdb-design.md`로 커밋.

## 재개 시 버려선 안 되는 원칙 (이것만은 살릴 것)

- **제로 오버헤드 역직렬화 (Pod 캐스팅 수준)**: 엔진이 바뀌어도 이 원칙은 유지한다.
- **컴파일 타임 쿼리 lowering (런타임 플래너 0)**: 스토리지가 원격이라도 쿼리 결정은 여전히 빌드 타임.
- **단일 writer + snapshot isolation**: object store의 conditional put과 자연스럽게 맞물린다.
- **`unsafe_code = forbid`**: 워크스페이스 전역 제약. 절대 완화하지 않는다.
- **`@model` 스키마 → 레코드 레이아웃 자동 유도**: 엔진이 바뀌어도 이 매핑은 그대로.
- **배치 커밋**: 원격 백엔드에서는 오히려 더 중요.

## 관련 파일·경로

- `fixtures/project-e2e/src/lib/db.orv` — `@model User / Post / Like` 사용 예. DB 재개 시 e2e 기준.
- `crates/orv-syntax/src/parser.rs:2050` — `parse_model_body`. 현재 `@annotation`을 전부 스킵 중. 재개 시 이 함수가 애너테이션을 실제로 캡처하도록 확장 필요.
- `crates/orv-resolve/src/resolver.rs:662` — `oql`을 stdlib 모듈로 등록하는 지점.
- `crates/orv-runtime/src/runner.rs` — 현재 HIR 인터프리터. 쿼리 lowering 결과를 실행하는 지점이 될 곳.

## 태스크 상태

이 브레인스토밍의 원래 체크리스트 중 다음까지 진행됨:

- [x] 프로젝트 컨텍스트 탐색
- [x] 명확화 질문 (Q1~Q6 완료)
- [x] 2~3개 접근 방식 제안 (CoW B+tree vs 하이브리드 vs Slotted + WAL)
- [ ] 디자인 섹션 발표 및 승인 — 섹션 1, 2 승인 완료. 섹션 3 작성 중 방향 수정으로 중단.
- [ ] 설계 문서 작성 및 커밋 — 이 중단 메모로 대체.
- [ ] 스펙 셀프 리뷰 — N/A (중단 상태).
- [ ] 사용자의 스펙 리뷰 대기 — N/A.
- [ ] writing-plans 스킬로 전환 — 재개 후.
