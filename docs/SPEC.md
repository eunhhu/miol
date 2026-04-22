# orv 언어 사양

> 이 문서는 orv 프로그래밍 언어의 공식 언어 사양서이다.
> 코드 예제의 언어 표시는 `orv`를 사용한다.

---

## 0. 플랫폼 철학

orv는 **단일 프로젝트 그래프를 공유하는 언어 + 컴파일러 + 에디터 + 런타임**이다. 네 레이어는 각각 정의되지만 같은 그래프 위에서 동작한다. 외부 VSCode/Neovim에서도 사용 가능하나, 자체 에디터에서 플랫폼의 시너지를 가장 풍부하게 경험할 수 있다 (§16 참조).

### 0.1 북극성 목표

> **비개발자가 AI 보조 없이 5시간 안에 쇼핑몰(결제·배송·회원 포함) 풀서비스를 배포할 수 있어야 한다.**

이 벤치마크는 설계 우선순위를 정하는 기준이다. 문법이 직관적이지 않거나, 불필요한 빌드 단계가 생기거나, 에디터가 상태를 숨긴다면 — 재검토한다.

### 0.2 설계 원칙

1. **우발적 복잡성 제거** — 빌드 체인/프레임워크 조합/라이브러리 선택은 본질이 아니므로 언어가 흡수한다
2. **도메인을 문법으로** — `@route`, `@db`, `@design`, `@sync` 같은 도메인 키워드가 자연어 수준으로 의도를 표현한다
3. **라이브 뷰 기본값** — 에디터는 프로젝트 상태를 정적으로 보여주는 것이 아니라 실시간으로 흐른다
4. **Zero-overhead, Zero-runtime** — 사용하지 않은 기능의 코드와 대응 런타임 계층은 번들에 존재하지 않는다
5. **컴파일타임 전체 분석** — 프로젝트 그래프를 항상 보유하므로 에디터/런타임/배포가 같은 진실을 공유한다

### 0.3 상호 참조

- 언어 문법: §2 ~ §9
- 도메인 시스템 (웹/서버): §10 ~ §11
- 컴파일러/최적화: §12 ~ §13
- 테스트/패키지: §14 ~ §15
- **통합 에디터: §16**

### 0.4 문서 계층과 우선순위

orv 문서는 다음 층위로 나뉜다.

1. **비전 문서 — `docs/README.md`**
   - 목표 사용자, 생산성 벤치마크, 설계 방향성을 설명한다.
2. **확정 사양 — `docs/SPEC.md`**
   - 현재 기준의 공식 문법, 의미론, 컴파일타임 규칙을 정의한다.
3. **실험 사양 / 탐색 예제 — `fixtures/default-syntax.orv`, `fixtures/plan/*.orv`**
   - 사용감 검토, 문법 압박 테스트, 미래 방향 탐색을 위한 예제다.
4. **구현 아키텍처 — `docs/ARCHITECTURE.md`**
   - 현재 컴파일러/크레이트 구조와 데이터 흐름을 설명한다.

문서 간 충돌이 있으면 **`SPEC.md`를 우선 기준**으로 해석한다. `fixtures/plan`과 `default-syntax.orv`는 중요한 설계 의도를 드러낼 수 있지만, 문법이나 의미론이 `SPEC.md`와 다르면 **탐색 중 예제**로 간주한다. `ARCHITECTURE.md`는 구현 구조의 기준 문서이며, 언어 의미론을 덮어쓰지 않는다.

예제를 읽을 때는 다음 순서를 권장한다.

- 방향성과 목표 이해: `docs/README.md`
- 공식 문법 확인: `docs/SPEC.md`
- 현재 구현 구조 확인: `docs/ARCHITECTURE.md`
- 사용감/실험 예제 탐색: `fixtures/default-syntax.orv`, `fixtures/plan/*.orv`, `fixtures/e2e/*.orv`

---

## 1. 표기법

이 사양서에서 사용하는 표기 규칙은 다음과 같다.

| 표기 | 의미 |
|------|------|
| `keyword` | 코드 내 키워드 또는 식별자 |
| `T` | 임의의 타입 매개변수 |
| `T?` | nullable 타입 (값이 `void`일 수 있음) |
| `T[]` | `T` 타입의 Vector |
| `(A, B)` | 튜플 타입 |
| `A \| B` | 유니온 타입 (A 또는 B) |
| `§N.M` | 본 문서 내 섹션 참조 |
| `// ...` | 코드 주석 또는 생략된 코드 |
| `// ❌` | 컴파일 에러가 발생하는 코드 |
| `// ✓` | 유효한 코드 |

사양에서 "~한다", "~이다"는 필수(MUST) 동작을 의미한다. "~할 수 있다"는 선택(MAY) 동작을 의미한다.

---

## 2. 어휘 구조

### 2.1 주석

한 줄 주석은 `//`로 시작한다. 블록 주석은 지원하지 않는다.

```orv
// 이것은 한 줄 주석이다.
let x: int = 42  // 인라인 주석도 가능하다.
```

### 2.2 식별자

식별자는 영문자 또는 밑줄(`_`)로 시작하며, 이후 영문자, 숫자, 밑줄을 포함할 수 있다.

### 2.3 키워드

다음은 orv의 예약 키워드이다.

```
let  mut  sig  const  function  async  await  return
if  else  when  for  in  while  break  continue
try  catch  throw  struct  enum  type  define  pub
import  true  false  void  as
```

### 2.4 리터럴

| 종류 | 예시 |
|------|------|
| 정수 | `42`, `-1`, `0` |
| 부동소수점 | `3.14`, `-0.5` |
| 불리언 | `true`, `false` |
| 문자열 | `"hello"`, `"보간: {name}"` |
| void | `void` |

문자열은 큰따옴표(`"`)로 감싸며, **기본적으로 보간(interpolation)을 지원한다.** 중괄호(`{}`) 안에 표현식을 넣으면 값이 삽입된다. 보간을 사용하지 않는 문자열은 컴파일타임에 일반 문자열로 최적화된다.

```orv
let name: string = "Alice"
let greeting: string = "Hello, {name}!"  // Hello, Alice!
let plain: string = "no interpolation"   // 컴파일타임에 일반 문자열로 최적화
```

**이스케이프 시퀀스:**

| 시퀀스 | 의미 |
|--------|------|
| `\n` | 줄바꿈 |
| `\t` | 탭 |
| `\\` | 백슬래시 리터럴 |
| `\{` | 중괄호 리터럴 `{` (보간 방지) |
| `\}` | 중괄호 리터럴 `}` |

```orv
let escaped: string = "가격: \{1000\}원"   // 가격: {1000}원
let path: string = "C:\\Users\\orv"        // C:\Users\orv
```

**정규식 리터럴:**

`r"패턴"플래그` 형태로 정규식을 작성한다. `r` 접두사로 시작하며, 닫는 따옴표 뒤에 플래그를 붙인다.

| 플래그 | 의미 |
|--------|------|
| `g` | global (전체 매칭) |
| `i` | case-insensitive |
| `m` | multiline |

```orv
let emailPattern = r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}"i
let digits = r"[0-9]+"g
let domain = r"[a-z\d].+\.[a-z]"gi
```

### 2.5 구분자 및 연산자

- 블록 구분: `{ }`
- 배열/인덱스: `[ ]`
- 호출/그룹: `( )`
- 속성 접근: `.`
- 범위: `..`, `..=`
- 산술: `+`, `-`, `*`, `/`, `%`, `**`
- 비교: `==`, `!=`, `<`, `>`, `<=`, `>=`
- 논리: `&&`, `||`, `!`
- 비트: `&`, `|`, `^`, `~`, `<<`, `>>`
- 대입: `=`, `+=`, `-=`
- 널 병합: `??`
- 타입 캐스팅: `as`
- 삼항: `? { } : { }`
- Spread/Rest: `...`

**Spread 연산자 (`...`):**

객체, 배열, 함수 파라미터에서 사용한다.

```orv
// 객체 spread
let base = { name: "Alice", age: 30 }
let updated = { ...base, age: 31 }          // { name: "Alice", age: 31 }

// 배열 spread
let arr1: int[] = [1, 2, 3]
let arr2: int[] = [...arr1, 4, 5]           // [1, 2, 3, 4, 5]

// Rest 파라미터
function sum(...nums: int[]): int -> {
  nums.reduce(0, (acc, n) -> acc + n)
}
@out sum(1, 2, 3)  // 6
```

---

## 3. 변수와 바인딩

### 3.1 let -- 불변 바인딩

`let`으로 선언한 변수는 재대입할 수 없다. 타입 어노테이션은 `:` 뒤에 명시한다.

```orv
let name: string = "John"
let age: int = 30
let height: float = 1.75
let isValid: bool = false
```

### 3.2 let mut -- 가변 바인딩

`let mut`으로 선언한 변수는 재대입이 가능하다.

```orv
let mut count: int = 0
count = 1
count = 2
```

### 3.3 let sig -- 반응형 바인딩

`let sig`로 선언한 변수는 반응형(reactive) 변수이다. 값이 변경되면 이 변수를 참조하는 모든 곳이 자동으로 갱신된다. sig는 DOM 전용이 아니며, 모든 도메인에서 범용으로 동작한다 (§12.2, §10.8 참조).

```orv
let sig score: int = 0
score = 100  // 의존하는 곳이 자동 갱신
```

sig 변수를 사용하는 도메인이 갱신 방식을 결정한다.

| 도메인 | 갱신 방식 |
|--------|-----------|
| `@out` | 콘솔 커서 위치로 돌아가 replace |
| `@html` | 해당 DOM 노드만 patch |
| `@fs` | 파일 자동 rewrite |
| `@route` | 반응하지 않음 (스냅샷 사용) |

```orv
let sig progress: int = 0
@out "진행률: {progress}%"   // "진행률: 0%"
progress = 50                // 같은 위치에 "진행률: 50%"로 replace
progress = 100               // "진행률: 100%"
```

### 3.4 const -- 상수

`const`는 컴파일타임 상수를 선언한다. 관례적으로 UPPER_SNAKE_CASE를 사용한다.

```orv
const PI: float = 3.14159
const MAX_RETRY: int = 3
const APP_NAME: string = "My App"
```

### 3.5 소유권 모델 — 참조 카운팅(RC) 기반

orv는 GC(가비지 컬렉터) 없이 **참조 카운팅(Reference Counting)** 기반으로 메모리를 관리한다. Rust식 lifetime/borrow checker는 사용하지 않는다. 기본 동작은 참조(reference)이며, 소유권 이동과 복사는 명시적으로 수행한다.

```orv
let str: string = "hello"
let str2 = str              // 참조 — RC 증가 (str과 같은 데이터를 가리킴)

let str3 = str.move()       // 소유권 이동 — str, str2 모두 사용 불가
// str 사용 시 컴파일 에러

let str4 = str3.copy()      // 깊은 복사 — 독립적인 복사본 생성
// str3, str4 모두 유효하며 독립적
```

**규칙 요약:**

| 문법 | 동작 | RC 변화 |
|------|------|---------|
| `let b = a` | 참조 (기본) | RC +1 |
| `let b = a.move()` | 소유권 이동 (원본 + 원본의 참조 모두 무효화) | 유효 참조 경로 기준 재계산 (기존 경로 무효화, 순 RC는 유지) |
| `let b = a.copy()` | 깊은 복사 | 새 RC 1 생성 |

함수 인자 전달도 기본적으로 참조이다.

```orv
function printName(user: User) -> {
  @out user.name   // 참조로 받음, 복사 없음
}
```

**컴파일타임 RC 최적화:** 컴파일러는 단일 소유자(참조가 하나뿐인 경우)에 대해 RC 오버헤드를 제거한다. 스코프 분석을 통해 참조 카운트 증감이 불필요한 경우를 감지하여, 런타임 비용 없이 메모리 안전성을 보장한다.

### 3.6 약한 참조와 순환 참조

RC 기반 모델은 순환 참조(cycle)를 자동으로 해제하지 못한다. orv는 이를 두 방향에서 해결한다.

**약한 참조 (`WeakRef<T>`):** `.weak()` 메서드로 생성한다. 약한 참조는 RC를 증가시키지 않으며, 대상이 해제된 뒤에는 `void`를 반환한다. 부모→자식은 강한 참조, 자식→부모는 약한 참조로 두는 것이 기본 패턴이다.

```orv
struct Node {
  value: int
  parent: WeakRef<Node>?   // 약한 참조 — 순환 방지
  children: Node[]          // 강한 참조
}

let root: Node = { value: 1, parent: void, children: [] }
let child: Node = { value: 2, parent: root.weak(), children: [] }
root.children.push(child)

// 접근
when child.parent {
  void -> @out "부모 해제됨"
  _ -> @out "부모 값: {child.parent.upgrade().value}"   // upgrade()로 강한 참조 임시 획득
}
```

**컴파일타임 순환 감지:** 컴파일러는 struct 필드 그래프에서 강한 참조 사이클을 감지하고 경고한다. `@hint allow=cycle`로 억제할 수 있으나, 해당 경로는 런타임 cycle collector 대상으로 등록된다.

**런타임 cycle collector:** 장기 실행 서버 프로세스의 경우 `gc.collect()`를 수동으로 호출하거나 `@hint gc=background`를 통해 백그라운드 스위퍼를 활성화할 수 있다. 기본값은 비활성이다 (zero-overhead 원칙).

### 3.7 스레드 안전 소유권

`spawn` 블록이 외부 값을 캡처하거나 `channel`로 값을 전송할 때, 컴파일러는 해당 값의 참조 전략을 자동으로 승격한다.

| 상황 | 자동 적용 |
|------|----------|
| 단일 스레드 내 참조 | 일반 RC (비원자) |
| `spawn` 경계 캡처 | Atomic RC로 자동 승격 |
| `channel.send(v)` | 소유권 이동(`move()` 자동) — 전송 후 원본 사용 금지 |
| `channel.send(v.copy())` | 깊은 복사 후 전송 |
| 공유 가변 상태 | `Mutex<T>` / `RwLock<T>` 명시 필요 |

```orv
let counter: Mutex<int> = Mutex(0)

for i in 0..4 {
  spawn {
    let guard = counter.lock()       // 락 획득
    guard.value = guard.value + 1
    // guard drop 시 락 해제
  }
}

// 읽기 다중, 쓰기 단일
let cache: RwLock<Map<string, User>> = RwLock(Map{})
let snapshot = cache.read().get("alice")   // 동시 읽기
cache.write().set("bob", user)             // 배타 쓰기
```

**경계 감지 원칙:** 단일 스레드에서만 도달 가능한 RC는 비원자 RC로 유지되며, 한 번이라도 `spawn` 경계를 넘으면 해당 타입 전체가 Atomic RC로 단일 변형된다. 혼합은 없다 — 이는 컴파일타임 escape analysis로 결정된다.

### 3.8 아레나 할당자

대규모 객체 그래프(씬 트리, CRDT 상태, AST 등)에서 개별 RC 해제는 깊이에 비례하는 일시 정지를 유발한다. `Arena<T>`는 모든 할당을 하나의 수명으로 묶어 일괄 해제한다.

```orv
let arena: Arena<Node> = Arena()
let root = arena.alloc({ value: 1, parent: void, children: [] })
let child = arena.alloc({ value: 2, parent: root.weak(), children: [] })

// arena drop 시 — 모든 Node를 O(1) 블록 단위로 해제
// RC 순회 없음, 개별 소멸자 호출 없음
```

**지연 해제 (`@hint drop=deferred`):** 렌더 루프 중 해제 비용이 프레임 버짓을 초과할 위험이 있는 경우, 해제를 다음 idle tick으로 지연한다. 게임/편집기 루프에서 유용하다.

---

## 4. 타입 시스템

### 4.1 원시 타입 (13종)

orv는 13종의 원시 타입을 제공한다.

| 타입 | 설명 | 범위 |
|------|------|------|
| `byte` | 부호 있는 8비트 정수 | -128 ~ 127 |
| `ubyte` | 부호 없는 8비트 정수 | 0 ~ 255 |
| `short` | 부호 있는 16비트 정수 | -32768 ~ 32767 |
| `ushort` | 부호 없는 16비트 정수 | 0 ~ 65535 |
| `int` | 부호 있는 32비트 정수 | -2^31 ~ 2^31-1 |
| `uint` | 부호 없는 32비트 정수 | 0 ~ 2^32-1 |
| `long` | 부호 있는 64비트 정수 | -2^63 ~ 2^63-1 |
| `ulong` | 부호 없는 64비트 정수 | 0 ~ 2^64-1 |
| `float` | 32비트 부동소수점 | IEEE 754 |
| `double` | 64비트 부동소수점 | IEEE 754 |
| `bool` | 불리언 | `true`, `false` |
| `string` | 문자열 | UTF-8 |
| `void` | 값 없음 | `void` |

`void`는 "값이 없음"을 나타내며, nullable 타입의 기본값으로 사용된다 (§4.8 참조).

타입은 `Type()` 함수로 확인할 수 있다.

```orv
let n: int = 42
@out Type(n)       // int
```

### 4.2 내장 복합 타입

원시 타입 외에 orv는 다음 내장 복합 타입을 제공한다. 별도 import 없이 사용 가능하다.

| 타입 | 설명 |
|------|------|
| `File` | 파일 핸들 (읽기/쓰기, 메타데이터 접근) |
| `Html` | HTML 문서 트리 (`@html` 도메인의 반환 타입) |
| `DomElement` | 단일 DOM 노드 참조 |
| `Time` | 시각/날짜 (`now()`, `today()` 등의 반환 타입) |
| `Blob` | 바이너리 데이터 |
| `Response` | HTTP 응답 객체 (`@fetch`의 반환 타입) |
| `Request` | HTTP 요청 객체 (`@request`의 타입) |
| `Error<T>` | 에러 래퍼 — `T`는 `throw`의 인자 타입이 결정 (§7.2 참조) |
| `unknown` | 임의의 타입 (타입 가드로 narrowing 필요) |
| `Async<T>` | 비동기 값 래퍼 — JavaScript의 `Promise<T>`에 대응 |
| `WeakRef<T>` | 약한 참조 (§3.6) |
| `Mutex<T>`, `RwLock<T>` | 동시성 잠금 (§3.7) |
| `Arena<T>` | 아레나 할당자 (§3.8) |
| `@textbuffer` | rope 기반 텍스트 버퍼 (§10.15) |
| SIMD 벡터 | `f32x4`, `f64x2`, `i32x4`, `i64x2`, `i16x8`, `i8x16` (§10.11) |

**메서드 규칙:** 사용자 정의 `struct`는 메서드를 가질 수 없지만, **내장 복합 타입과 원시 타입은 언어가 제공하는 메서드를 갖는다.** 위 표의 타입들, 그리고 `string`, `T[]`, `Map<K,V>`, `Set<T>` 등은 dot-notation 호출이 가능하다. 이는 사용자 코드의 struct 메서드 금지 규칙(§4.6)과 별개이며, 컴파일러는 내장 메서드를 intrinsic으로 인식해 최적화한다.

```orv
let buf = @textbuffer()            // 생성자 호출
buf.insert(0, "Hello")             // 내장 메서드
let lock = Mutex(0)                // 생성자 호출
let guard = lock.lock()            // 내장 메서드
let arr: int[] = [1, 2, 3]
arr.filter((x) -> x > 1)           // 내장 메서드 (§4.3)
```

```orv
let file: File = await @fs.read ./data.txt
let page: Html = @html { @body { @p "hello" } }
let res: Response = await @fetch GET "https://api.example.com/data"
let timestamp: Time = now()
let data: Blob = await @fs.read ./image.png
```

### 4.3 컬렉션 타입

| 타입 | 문법 | 예시 |
|------|------|------|
| Vector | `T[]` 또는 `Vector<T>` | `[1, 2, 3]` |
| Tuple | `(A, B)` 또는 `Tuple<A, B>` | `(1, "hello")` |
| Object | 리터럴 `{ key: value }` | `{ name: "John", age: 30 }` |
| Set | `Set<T>` | `Set{1, 2, 3}` |
| Map | `Map<K, V>` | `Map{"alice": 95}` |

```orv
let numbers: int[] = [1, 2, 3, 4, 5]
let person = { name: "John", age: 30 }
let scores: Map<string, int> = Map{"alice": 95, "bob": 87}
```

Vector는 `filter`, `map`, `reduce`, `find`, `some`, `every`, `sort`, `push`, `concat` 메서드를 제공하며, 체이닝이 가능하다. Set은 `union`, `intersect`, `difference`를 제공한다. Map은 `keys()`, `values()`, `has()`를 제공한다.

```orv
let result: int = [1, 2, 3, 4, 5]
  .filter((x) -> x % 2 == 1)
  .map((x) -> x * 10)
  .reduce(0, (acc, x) -> acc + x)   // 90
```

`Type()` 함수로 기존 값의 타입을 가져와 재사용할 수 있다: `let newMap: Type(scores) = Map{"charlie": 92}`

### 4.4 Enum

enum은 명명된 상수 집합을 정의한다.

```orv
// 인라인
enum SizeUnit { Px = "px", Em = "em", Rem = "rem" }

// 멀티라인
enum Status {
  Pending = 0
  Running = 1
  Completed = 2
}

@out Status.Pending   // 0
```

enum은 `when` 패턴 매칭과 함께 사용할 수 있다 (§6 참조).

### 4.5 Type Alias (유니온 타입)

`type` 키워드로 타입 별칭을 선언한다. 리터럴 타입과 유니온을 지원한다.

```orv
type HttpMethod = "GET" | "POST" | "PUT" | "DELETE"
type Size = "{uint}{SizeUnit}" | "auto" | uint
type Result<T, E> = { ok: true, value: T } | { ok: false, error: E }
```

`Result<T, E>`는 `ok` 필드를 기준으로 분기하는 tagged union이다.

### 4.6 Struct

**사용자 정의 struct는 데이터만 담는 순수 구조체이다.** 메서드를 가질 수 없으며, trait/interface/impl 블록도 없다. 기능이 필요하면 함수(§5)나 도메인(§9)으로 구현한다. 다형성은 도메인 시스템으로 해결한다.

> **내장 타입과의 차이:** 이 규칙은 **사용자 코드의 struct**에 한정된다. 내장 복합 타입(§4.2의 표)과 원시 타입은 언어가 제공하는 메서드를 가지며, `str.toLowerCase()`, `arr.filter(...)`, `buf.insert(...)` 같은 호출이 가능하다.

필드 접근은 dot(`user.name`)과 bracket(`user["name"]`) 모두 가능하며, 멀티라인 오브젝트에서 쉼표는 생략할 수 있다.

```orv
struct User {
  name: string
  age: int
}

let user: User = { name: "John", age: 30 }
```

struct는 중첩, 인라인 오브젝트 배열, nullable 필드를 지원한다.

```orv
struct Post {
  title: string
  tags: {name: string}[]  // 인라인 오브젝트 배열
  creator: User
  nullable: string?       // nullable 필드
}
```

### 4.7 제네릭

struct와 함수에 타입 매개변수를 사용할 수 있다.

```orv
struct Box<T> { value: T }
struct Pair<A, B> { first: A, second: B }

let intBox: Box<int> = { value: 42 }

function identity<T>(x: T): T -> x
function map<T, U>(items: T[], fn: Function<T, U>): U[] -> {
  let mut result: U[] = []
  for item in items {
    result.push(fn(item))
  }
  result
}
```

### 4.8 Nullable (?)

타입 뒤에 `?`를 붙이면 nullable 타입이 된다. nullable 변수는 `void` 값을 가질 수 있다.

```orv
let maybe: string? = void
let sure: string? = "exists"
```

널 병합 연산자 `??`로 기본값을 지정할 수 있다.

```orv
let fallback: string = maybe ?? "default"  // "default"
```

### 4.9 타입 캐스팅과 변환

**캐스팅** -- `as` 키워드로 타입 간 캐스팅을 수행한다.

```orv
let n: int = 8
let new_n: short = n as short
```

**변환** -- `.from()` 정적 메서드로 타입을 변환한다.

```orv
let fv: float = 3.15
let new_f: int = int.from(fv)          // 3
let sv: string = string.from(fv)       // "3.15"
```

### 4.10 스키마 검증

orv의 타입 선언은 그대로 런타임 검증 스키마가 된다. 리터럴 제약을 타입 시그니처에 붙이면 컴파일타임 체크와 런타임 validator가 자동 생성된다.

#### 제약조건

**범위 제약** -- `(min..max)` 형태로 길이 또는 값의 범위를 제한한다.

```orv
let ok_name: string(3..50) = "Alice"    // 길이 3~50
let age: int(0..120) = 25               // 값 0~120
let pin: string(4) = "1234"             // 정확히 4자
let tiny: int(0..=9) = 5                // 0~9 (inclusive end)
```

#### 키워드 인자

`(key=value, ...)` 형태로 제약을 명시한다.

```orv
let code: string(min=4, max=10) = "abcd"
let port: int(min=1024, max=65535) = 8080
let username: string(min=3, max=20, pattern="^[a-z0-9_]+$") = "alice"
```

#### 컬렉션 제약

배열에도 제약을 적용할 수 있다.

```orv
let tags: string[](0..10) = ["a", "b", "c"]            // 배열 길이 0~10
let uniqueTags: string[](unique, 1..5) = ["x"]         // 중복 불가, 길이 1~5
```

#### 패턴 타입

리터럴 타입 문법을 활용하여 패턴 타입을 정의한다. 표준 패턴 타입(`Email`, `URL`, `UUID`, `IPv4`, `ISODate`)은 내장되어 있다.

```orv
type Email = "{string}@{string}.{string}"
type URL = "{string}://{string}"
type IPByte = int(0..255)
type IPv4 = "{IPByte}.{IPByte}.{IPByte}.{IPByte}"

let email: Email = "user@orv.dev"          // ✓
let bad_email: Email = "not an email"      // ❌
```

#### struct 스키마

struct 필드에 직접 제약을 적용할 수 있다.

```orv
struct User {
  name: string(min=1, max=50)
  email: "{string}@{string}.{string}"
  age: int(0..120)
  role: "admin" | "member" | "guest"
  tags: string[](unique, 0..10)
  bio: string(max=500)?                  // nullable + 길이 제한
}
```

#### 검증 메서드

스키마가 정의된 타입에는 다음 정적 메서드가 자동 생성된다.

| 메서드 | 반환 타입 | 설명 |
|--------|-----------|------|
| `.validate(unknown)` | `bool` | 통과 여부만 반환 |
| `.parse(unknown)` | `T` | 성공 시 T, 실패 시 throw |
| `.safeParse(unknown)` | `Result<T, ValidationError[]>` | 성공/에러 객체 반환 (throw 없음) |
| `.errors(unknown)` | `ValidationError[]` | 모든 필드 에러 수집 |
| `.is(unknown)` | `bool` | 타입 가드 (`when`에서 narrowing) |

```orv
let isValid: bool = User.validate(input)
let user: User = User.parse(input)
let result: Result<User, ValidationError[]> = User.safeParse(input)

// .is -- when에서 타입 가드
when input {
  User.is -> @out input.name   // input이 User로 narrowing
  _ -> @out "User가 아님"
}
```

#### transform 제약

`trim`, `lower`, `upper` 등 변환 토큰은 `parse` 시 자동으로 적용된다.

```orv
struct SignupForm {
  email: string(trim, lower)               // 공백 제거 + 소문자
  username: string(trim, min=3, max=20)
}

let raw: unknown = { email: "  USER@ORV.DEV  ", username: " alice ", password: "12345678" }
let form: SignupForm = SignupForm.parse(raw)
@out form.email   // "user@orv.dev"
```

#### where 절

제약 문법으로 표현할 수 없는 커스텀 조건은 `where`로 정의한다. `$`는 현재 값을 가리킨다.

```orv
type EvenInt = int where $ % 2 == 0
type StrongPassword = string(min=8) where
  $.contains(r"[A-Z]") &&
  $.contains(r"[0-9]") &&
  $.contains(r"[^a-zA-Z0-9]")

struct Registration {
  email: Email
  password: StrongPassword
  age: int where $ >= 13              // 인라인 where
}
```

#### ValidationError 타입

검증 에러의 표준 타입이다. import 없이 사용 가능하다.

```orv
struct ValidationError {
  path: string           // "user.address.zip"
  code: string           // "too_short", "pattern_mismatch", "out_of_range" 등
  message: string        // 사람이 읽을 수 있는 메시지
  expected: string?      // 기대값 설명
  actual: unknown?       // 실제 값
}
```

#### 제약 요약 표

| 문법 | 의미 |
|------|------|
| `(N)` | 정확히 N (길이/값) |
| `(N..M)` | 범위 N <= x < M |
| `(N..=M)` | inclusive 범위 N <= x <= M |
| `(min=N, max=M)` | 키워드 인자 |
| `(pattern="...")` | 정규식 패턴 |
| `(unique)` | 컬렉션 중복 불가 |
| `(trim, lower, upper)` | 문자열 변환 토큰 |
| `(nonempty)` | 빈 값 불가 |

토큰, 키워드, 범위는 자유롭게 조합할 수 있다: `string(trim, min=3, max=20, pattern="^[a-z0-9]+$")`. 모든 제약은 컴파일타임(리터럴)과 런타임(`parse`/`validate`)에서 모두 동작하며, 사용하지 않은 검증 코드는 DCE로 번들에서 제거된다 (§13.1 참조).

---

## 5. 함수

### 5.1 선언

함수는 `function` 키워드로 선언한다. 반환 타입은 `:` 뒤에 명시하고, 본문은 `->` 뒤에 작성한다.

```orv
function add(a: int, b: int): int -> {
  return a + b
}
@out add(1, 2)  // 3
```

### 5.2 암시적 반환

블록의 마지막 표현식이 자동으로 반환값이 된다. `return`을 생략할 수 있다.

```orv
function triple(x: int): int -> {
  x * 3
}
@out triple(5)  // 15
```

### 5.3 Expression body

본문이 단일 표현식이면 블록 없이 작성할 수 있다.

```orv
function double(x: int): int -> x * 2
```

### 5.4 함수 타입과 클로저

함수는 first-class 값이다. `Function<Params, Return>` 타입으로 표현한다. 클로저는 외부 스코프의 변수를 캡처한다.

```orv
let fn: Function<int, int> = (x) -> x * 2
@out fn(5)  // 10

// 고차 함수
function apply(x: int, f: Function<int, int>): int -> f(x)
@out apply(5, (x) -> x + 10)  // 15

// 클로저
function makeCounter(): Function<void, int> -> {
  let mut count: int = 0
  () -> { count = count + 1; count }
}
```

---

## 6. 제어 흐름

### 6.1 if/else

```orv
if num > 5 {
  @out "greater"
} else if num < 5 {
  @out "less"
} else {
  @out "equal"
}
```

한 줄 조건문은 `:` 뒤에 작성한다.

```orv
if num > 5 : @out "한 줄 조건문"
```

### 6.2 삼항 연산자

```orv
num > 5 ? {
  @out "greater"
} : {
  @out "not greater"
}
```

삼항 연산자는 값을 반환할 수 있다.

```orv
let message: string = num > 5 ? "big" : "small"
```

### 6.3 when (패턴 매칭)

`when`은 값에 대한 패턴 매칭을 수행한다. `_`는 기본(fallback) 분기이다. 범위(`25..30`), 부정(`!5`), 포함(`in`), 값 참조(`$`) 패턴을 지원한다.

```orv
when num {
  10 -> @out "num is 10"
  25..30 -> @out "num is between 25 and 30"
  !5 -> @out "num is not 5"
  _ -> @out "something else"
}

// Vector, Object, String에도 사용 가능
when vec {
  in 4 -> @out "contains 4"
  $.length > 3 -> @out "more than 3 elements"
  _ -> @out "default"
}
```

### 6.4 for/while 루프

**range 반복:**

```orv
for i in 0..10 {
  @out "i: {i}"
}
```

**컬렉션 순회:**

```orv
for item in [1, 2, 3] {
  @out "item: {item}"
}
```

**인덱스와 함께:**

```orv
for (item, index) in ["apple", "banana", "cherry"] {
  @out "{index}: {item}"
}
```

**while 루프:**

```orv
let mut counter: int = 0
while counter < 5 {
  counter = counter + 1
}
```

`break`와 `continue`를 지원한다.

---

## 7. 비동기

### 7.1 async/await

`async` 키워드로 비동기 함수를 선언하고, `await`로 결과를 기다린다.

```orv
async function fetchData(): string -> {
  await sleep(1000)
  "data"
}
let data: string = await fetchData()
```

### 7.2 에러 모델 (throw / try-catch)

orv의 에러 모델은 JavaScript 스타일을 따른다. `?` 연산자는 없다.

#### throw와 Error\<T\>

`throw`의 인자 타입이 `Error<T>`의 `T`를 결정한다. `Error<T>`는 `.msg` 필드로 에러 값에 접근한다.

```orv
throw "something went wrong"
// → Error<string>, catch에서 e.msg는 string

throw { code: 404, msg: "not found" }
// → Error<{code: int, msg: string}>, catch에서 e.msg는 {code: int, msg: string}
```

#### try-catch

`try-catch`로 예외를 처리한다. `catch`에서 에러 타입을 명시할 수 있다.

```orv
try {
  let risky: int = int.from("not a number")
} catch error {
  @out "에러 발생: {error}"
}

// 타입 명시
try {
  let user: User = User.parse(input)
} catch err: Error<ValidationError[]> {
  for e in err.msg {
    @out "{e.path}: {e.message}"
  }
}

// 구조화된 에러
try {
  throw { code: 403, msg: "forbidden" }
} catch err: Error<{code: int, msg: string}> {
  @out "에러 코드: {err.msg.code}"  // 403
}
```

#### 패닉 — try 없는 throw

**`try` 블록 없이 `throw`를 실행하면 패닉(panic)이 발생하여 프로세스가 즉시 종료된다.** 이는 복구 불가능한 에러를 의미한다.

```orv
// 패닉 — 프로세스 종료
function mustBePositive(n: int) -> {
  if n <= 0 { throw "n must be positive" }  // try 없으면 패닉
}

// 안전한 사용 — try로 감싸기
try {
  mustBePositive(-1)
} catch e: Error<string> {
  @out "잡힌 에러: {e.msg}"  // "n must be positive"
}
```

#### 자동 전파 (escape analysis)

`?` 연산자를 도입하지 않은 이유는, orv가 **호출 그래프 전체를 컴파일타임에 분석**할 수 있기 때문이다. 함수 본문에 `throw`(직접 또는 간접)가 있으면 컴파일러는 해당 함수를 자동으로 "may-throw"로 표시하고, 호출자에 **암시적 전파 경로**를 삽입한다.

```orv
function parseAge(s: string): int -> int.from(s)   // may-throw (int.from이 throw)
function greet(s: string) -> {
  let age = parseAge(s)      // 암시적 전파 — 실패 시 상위로 throw
  @out "age: {age}"
}

// 호출 그래프 상 가장 가까운 try가 catch
try {
  greet("not-a-number")
} catch e { @out "잡힘: {e.msg}" }
```

**컴파일타임 보장:** 컴파일러는 각 함수의 "에러 시그니처"를 추론한다. `@route` 최상단, `spawn { }` 본문, `@job` 핸들러, `main` 본문 중 어느 곳까지 에러가 전파될 수 있는지 그래프로 파악한다. 이 지점에 try가 없다면 경고를 띄운다 (실행 시 패닉 경로).

**선언적 catch-all:** 라우트/job/spawn 경계에는 `@catch` 수식자를 붙여 명시적으로 경계를 설정할 수 있다.

```orv
@route POST /api/upload @catch (e) -> @respond 500 { error: e.msg } {
  let file = await parseBody()      // 여기서 throw 가능
  await @storage.put(file.path, file.blob)
  @respond 201 {}
}

@job ingest @catch (e) -> retryLater() (payload: Upload) -> {
  await processUpload(payload)
}
```

**왜 `?` 없음:** Rust식 `?`는 매 호출마다 표기를 강요해 보일러플레이트를 증가시킨다. orv는 전체 프로젝트를 알기 때문에 전파가 암시로 충분하며, 경계는 `try` 또는 `@catch`로 한 번만 선언하면 된다.

### 7.3 동시성 — 멀티스레드 + 채널

orv는 단일 스레드 이벤트 루프가 아닌 **멀티스레드 런타임**을 사용한다. Go/Rust 스타일의 동시성 프리미티브를 제공한다.

#### spawn

`spawn`으로 새 태스크를 생성한다. 태스크는 멀티스레드 런타임 위에서 스케줄링된다.

```orv
spawn {
  let result = await heavyComputation()
  @out "완료: {result}"
}

// 반환값 받기
let handle = spawn {
  await processData(data)
}
let result = await handle
```

#### channel

`channel`로 태스크 간 메시지를 주고받는다. 타입 안전한 통신을 보장한다.

```orv
let (tx, rx) = channel<string>()

spawn {
  tx.send("hello from spawned task")
}

let msg: string = await rx.recv()
@out msg  // "hello from spawned task"
```

#### 병렬 처리 패턴

```orv
// 여러 작업 병렬 실행
let (users, posts, stats) = await (
  fetchUsers(),
  fetchPosts(),
  fetchStats()
)

// 채널로 워커 풀 구현
let (tx, rx) = channel<int>()
for i in 0..4 {
  spawn {
    for item in rx {
      processItem(item)
    }
  }
}
for item in items { tx.send(item) }
```

---

## 8. 모듈 시스템

### 8.1 import/pub

orv의 모듈 시스템은 파일 기반 스코핑을 사용한다. 디렉토리 구조가 곧 모듈 경로이다.

```
src/
  main.orv
  models/
    user.orv    -- pub struct User, pub function createUser
    post.orv    -- pub struct Post
  utils/
    format.orv  -- pub function formatDate
```

**단일 import:**

```orv
import models.user.User
import models.user.createUser
```

**선택적 import:**

```orv
import models.post.{Post, PostCard}
```

**전체 import:**

```orv
import utils.format.*
```

### 8.2 pub 키워드

`pub`이 붙은 선언만 외부에서 import할 수 있다. `pub`이 없으면 해당 파일 스코프에서만 접근 가능하다.

```orv
pub struct User { ... }         // 외부에서 import 가능
struct InternalCache { ... }    // 파일 내부에서만 사용
```

`pub`은 `struct`, `enum`, `function`, `define`, `const`에 모두 적용할 수 있다.

### 8.3 표준 라이브러리

다음 모듈은 orv 표준 라이브러리에 내장되어 있으며, 별도 import 없이 사용할 수 있다.

#### jwt

JSON Web Token 생성과 검증을 제공한다.

```orv
let token: string = jwt.sign({ userId: 1, role: "admin" }, @env.SECRET)
let payload = jwt.verify(token, @env.SECRET)   // 실패 시 throw
```

#### hash

해시 함수를 제공한다. 비밀번호 해싱에는 `hash.password`(argon2 기반)를 사용한다.

```orv
let hashed: string = hash.sha256("data")
let passwordHash: string = await hash.password("mypassword")
let isValid: bool = await hash.verify("mypassword", passwordHash)
```

#### crypto

암호화/복호화, 랜덤 생성 등 암호화 프리미티브를 제공한다.

```orv
let key = crypto.generateKey()
let encrypted: Blob = crypto.encrypt(data, key)
let decrypted = crypto.decrypt(encrypted, key)
let uuid: string = crypto.uuid()
let random: int = crypto.randomInt(1, 100)
```

#### session

인메모리 세션 DB를 제공한다. Redis를 대체하는 자체 구현이며, Elixir ETS와 같은 철학을 따른다. 키-값 저장, TTL, 원자적 증감을 지원한다.

```orv
await session.set("user:1", { name: "Alice", role: "admin" })
let user = await session.get("user:1")
await session.del("user:1")

// 원자적 증감 + TTL
let count: int = await session.incr("rate:127.0.0.1:/api")
await session.expire("rate:127.0.0.1:/api", 60)   // 60초 TTL

// TTL과 함께 설정
await session.set("token:abc", tokenData, ttl=3600)
```

#### db

orv-db는 프로젝트에 최적화된 자체 DB 시스템이다 (§11.8 참조).

#### vault

시크릿 관리 전용 모듈이다. `@env`는 개발 편의용이며, 프로덕션 시크릿(결제 키, 서명 키, OAuth 클라이언트 시크릿 등)은 반드시 `vault`를 통해 접근한다.

```orv
// 시크릿 조회 — 인가된 스코프에서만 가능
let signKey: string = await vault.get("payment/sign-key")

// 로테이션 후크
vault.onRotate("payment/sign-key", (oldKey, newKey) -> {
  @out "키 로테이션 완료"
})

// HSM 바인딩 — PKCS#11
let hsm = vault.hsm(slot=0)
let signature: Blob = await hsm.sign(digest, keyId="prod-1")
```

컴파일러는 `vault.get` 호출 경로를 분석하여 시크릿 **스코프 범위**를 생성한다. 해당 스코프 밖에서 같은 키를 요청하면 컴파일 에러이다. 이는 PCI-DSS/SOC2 감사 범위를 언어 레벨에서 축소한다.

#### oauth

OAuth2/OIDC 표준 흐름을 내장한다. 인가 코드 + PKCE, 리프레시 토큰 회전, 디바이스 코드 플로우를 지원한다.

```orv
let google = oauth.provider {
  issuer: "https://accounts.google.com"
  clientId: @env.GOOGLE_CLIENT_ID
  clientSecret: vault.get("oauth/google/secret")
  scopes: ["openid", "email", "profile"]
}

@route GET /auth/google/start {
  @redirect google.authorizeUrl(state=crypto.uuid())
}

@route GET /auth/google/callback {
  let session: oauth.Session = await google.exchange(@query.code, @query.state)
  // session.accessToken, session.refreshToken, session.idToken
  @respond 200 { user: session.idToken.claims }
}
```

#### smtp / imap

메일 프로토콜 바인딩이다. DKIM 서명, SPF/DMARC 검증, STARTTLS, MIME 인코딩을 표준으로 지원한다.

```orv
// 발송
await smtp.send {
  from: "noreply@orv.dev"
  to: ["alice@example.com"]
  subject: "환영합니다"
  html: @render(@WelcomeMail name="Alice")
  dkim: { selector: "mail", privateKey: vault.get("mail/dkim") }
}

// 수신 (IDLE)
imap.watch {
  mailbox: "INBOX"
  @on new-message {
    let mail = await imap.fetch(@packet.uid)
    await processIncoming(mail)
  }
}
```

#### audit

불변 감사 로그이다. 구조화된 이벤트를 append-only 저장소(WAL 기반)로 기록하며, 해시 체인으로 변조를 감지한다. 금융/헬스케어/규제 환경 필수.

```orv
audit.log "payment.charged" {
  actor: @context.payload.id
  resource: "order/{orderId}"
  amount: amount
  ip: @request.ip
}

// 감사 검색
let events = await audit.query {
  @where actor=userId
  @where type="payment.charged"
  @range from=startDate to=endDate
}
```

#### @ffi / @unsafe

시스템 콜, 네이티브 라이브러리, 하드웨어 가속 바인딩이 필요할 때 사용한다. **FFI 영역은 orv 안전성 보증에서 제외된다.** `@ffi` 블록으로 외부 심볼을 선언하고, 호출은 반드시 `@unsafe` 블록 안에서만 가능하다.

```orv
@ffi "C" {
  function tun_create(name: string): int
  function tun_write(fd: int, packet: Blob): int
}

// 호출 — @unsafe 밖에서는 컴파일 에러
@unsafe {
  let fd = tun_create("orv0")
  tun_write(fd, packet)
}
```

플랫폼별 심볼은 `@ffi "C" platform=linux { ... }`처럼 속성으로 지정하며, 매칭되지 않은 타겟에서는 호출부가 dead code로 처리된다. 사용하는 모듈은 `orv.toml`의 `[ffi]` 섹션에 선언해야 빌드가 허용된다.

컴파일러는 `@ffi` 선언에 대해 ABI 시그니처 검증을 수행하고, `@unsafe` 경계를 탈출하는 포인터/핸들에 대해 RC 추적을 중단한다. FFI 리소스는 명시적으로 해제하거나 `Drop` 가드로 감싸야 한다.

---

## 9. 도메인 시스템

도메인은 orv의 핵심 확장 메커니즘이다. `define`으로 선언하고 `@`으로 호출하면, 호출 자리에 결과가 그대로 삽입된다 (매크로와 유사).

### 9.1 도메인 정의 (define)

`define` 키워드로 도메인을 선언한다. `->` 뒤에 반환 블록 또는 표현식을 작성한다.

```orv
define Pi() -> 3.14159
define HelloBlock() -> {
  @out "Hello from domain!"
}
```

Void scope 자동 출력 규칙(§12.2)은 도메인에도 동일하게 적용된다.

### 9.2 도메인 호출 (@)

`@` 접두사로 도메인을 호출한다. 도메인의 반환값이 호출 자리에 확장된다.

```orv
@out @Pi           // 3.14159
@HelloBlock        // "Hello from domain!" 출력
```

도메인은 어떤 타입이든 반환할 수 있으며, 다른 도메인을 반환할 수도 있다.

```orv
define Slug(title: string) -> {
  title.toLowerCase().replace(" ", "-")
}
let slug: string = @Slug title="Hello World"  // "hello-world"
```

### 9.3 Property (key=value 매개변수)

property는 반드시 `key=value` 형태로 전달한다. key 없이 전달하면 token으로 처리된다 (§9.4 참조).

```orv
define Greet(name: string, greeting: string?) -> {
  @out "{greeting ?? "Hello"}, {name}!"
}

@Greet name="Alice"                       // Hello, Alice!
@Greet name="Bob" greeting="Hi"           // Hi, Bob!
```

nullable property는 타입 뒤에 `?`를 붙인다.

### 9.4 Token (키 없는 값, 항상 배열)

token은 키 없이 전달되는 값이며, 항상 배열로 수집된다. `token` 블록 또는 `token` 인라인으로 선언한다.

```orv
define Log(label: string?) -> {
  token {
    message: string    // 실제 타입은 string[]
  }
  @out "[{label ?? "LOG"}] {message[0]}"
}

@Log "서버 시작" label="INFO"    // [INFO] 서버 시작
```

여러 token 타입을 사용하면 패턴에 따라 자동 분류된다.

```orv
define Style() -> {
  token {
    size: "{uint}px"    // "{uint}px"[] 패턴 매칭
    color: string       // string[] 나머지
  }
}

@Style 16px 24px red blue 32px
// sizes: ["16px", "24px", "32px"]
// colors: ["red", "blue"]
```

property와 token의 순서는 자유이다. 단, `{}` 블록은 항상 마지막에 위치한다.

### 9.5 @content 지시어

`@content`를 사용하면 호출부의 `{}` 블록 내용이 해당 위치에 삽입된다.

```orv
define Section(title: string) -> {
  @out "=== {title} ==="
  @content
  @out "=== /{title} ==="
}

@Section title="Introduction" {
  @out "이 섹션의 내용입니다."
}
// === Introduction ===
// 이 섹션의 내용입니다.
// === /Introduction ===
```

`@content`가 없는 도메인에 `{}` 블록을 전달하면 무시된다 (에러가 아님). `@content`는 무조건 가장 가까운 부모의 slot을 받아온다.

### 9.6 중첩 도메인 (Parent.Child)

도메인 내부에서 `define`으로 하위 도메인을 선언할 수 있다. 외부에서는 `Parent.Child` 경로로 접근하고, 해당 도메인 스코프 내에서는 `Child`만으로 접근 가능하다. 깊은 중첩도 가능하다 (`@App.Nav.Item`).

```orv
define Layout(title: string) -> {
  @content
  define Header() -> { @out "[ HEADER: {title} ]" }
  define Footer(copyright: string?) -> { @out "[ FOOTER: {copyright ?? "2026"} ]" }
}

@Layout title="Home" { @Header; @Footer }   // 스코프 내 직접 접근
@Layout.Header                               // 외부에서 경로 접근
```

### 9.7 도메인 반환 타입

도메인은 숫자, 문자열, 다른 도메인 등 어떤 타입이든 반환할 수 있다.

```orv
define Random() -> 42
define Page(title: string) -> @html {
  @head { @title "{title}" }
  @body { @content }
}

@out Type(@Random)   // int
@out Type(@Page)     // Html
```

### 9.8 pub 키워드

`pub define`으로 선언하면 다른 파일에서 import하여 사용할 수 있다 (§8.2 참조).

```orv
pub define PublicCard(title: string) -> @div {
  @h2 "{title}"
  @content
}
```

### 9.9 도메인 합성

도메인끼리 자유롭게 합성할 수 있다. 조건부 합성(`if`), 반복 합성(`for`)도 가능하다.

```orv
define Container() -> @div { @content }
define Title(text: string) -> @h1 "{text}"

define Hero(title: string, subtitle: string) -> @Container {
  @Title text="{title}"
  @p "{subtitle}"
}
```

상세 예제: `fixtures/plan/03-domains.orv`

---

## 10. 웹 도메인

웹 도메인은 브라우저 런타임에 컴파일되는 프런트엔드 코드를 다룬다.

### 10.1 @html 루트 도메인

`@html`은 HTML 문서 전체를 표현하는 루트 도메인이다. `@head`와 `@body`를 포함한다.

```orv
pub define HelloPage() -> @html {
  @head {
    @title "Hello"
    @meta charset="utf-8"
  }
  @body {
    @h1 "Hello, Orv!"
  }
}
```

번들 규모는 실제로 사용한 기능에 따라 결정된다 (§13.1 참조).

- sig 없음, await 없음, 이벤트 없음 -- 순수 정적 HTML (JS/WASM 없음)
- 이벤트 핸들러만 사용 -- 최소 이벤트 바인딩 코드만 포함
- sig 사용 -- 반응형 DOM patch 런타임 포함

### 10.2 HTML 태그

#### 텍스트/헤딩

`@h1` ~ `@h6`, `@p`, `@span`, `@strong`, `@em`, `@small`, `@mark`, `@code`, `@pre`를 지원한다.

```orv
@h1 "가장 큰 헤딩"
@p {
  "문단 안에 "
  @strong "강조"
  "나 "
  @em "기울임"
  "도 넣을 수 있음."
}
```

#### 링크

```orv
@a href="/" "홈으로"
@a href="https://orv.dev" target="_blank" rel="noopener" "외부 링크"
```

#### 블록

`@div`, `@section`, `@article`, `@aside`, `@header`, `@footer`, `@main`을 지원한다.

```orv
@div {
  class="container"
  @p "일반 블록 컨테이너"
}
```

#### 리스트

```orv
@ul {
  @li "첫 번째"
  @li "두 번째"
}

@ol {
  @li "하나"
  @li "둘"
}

@dl {
  @dt "용어"
  @dd "정의"
}
```

#### 폼

`@form`, `@input`, `@label`, `@button`, `@textarea`, `@select`, `@option`을 지원한다. boolean 속성(`disabled`, `checked`, `required` 등)은 값 없이 적어도 된다.

```orv
@form onSubmit={(e) -> e.preventDefault()} {
  @label for="email" "이메일"
  @input type=email id=email name=email placeholder="you@example.com" required
  @button type=submit "전송"
}

@select name=country {
  @option value="" disabled selected "선택..."
  @option value="kr" "한국"
}
```

#### 테이블

```orv
@table {
  @thead {
    @tr { @th "ID"; @th "이름" }
  }
  @tbody {
    for user in users {
      @tr { @td "{user.id}"; @td "{user.name}" }
    }
  }
}
```

### 10.3 특수 미디어 태그

| 태그 | 용도 |
|------|------|
| `@img` | 이미지 |
| `@video` | 비디오 (`@source`, `@track` 포함) |
| `@audio` | 오디오 |
| `@canvas` | 2D/WebGL 캔버스 |
| `@svg` | SVG (네임스페이스 자동 처리) |
| `@iframe` | 외부 문서 임베드 |

```orv
@img src="/hero.png" alt="히어로" loading=lazy
@video src="/intro.mp4" controls width=640 height=360

@svg viewBox="0 0 100 100" width=100 height=100 {
  @circle cx=50 cy=50 r=40 fill="#1a1a2e"
}

@canvas onMount={(el) -> {
  let ctx = el.getContext("2d")
  ctx.fillRect(0, 0, 100, 100)
}}
```

### 10.4 속성 문법

모든 속성은 `key=value` 형태로 통일된다.

| 형태 | 예시 |
|------|------|
| 리터럴 문자열 | `type=email`, `href="/home"` |
| 표현식 | `value={count}`, `onClick={() -> ...}` |
| Boolean | `disabled`, `required`, `checked` (값 없이 적으면 `true`) |

### 10.5 이벤트 핸들러

`on` 접두사로 이벤트 핸들러를 바인딩한다.

| 카테고리 | 이벤트 |
|----------|--------|
| 마우스 | `onClick`, `onDblClick`, `onMouseDown`, `onMouseUp`, `onMouseMove`, `onMouseEnter`, `onMouseLeave` |
| 키보드 | `onKeyDown`, `onKeyUp`, `onKeyPress` |
| 입력 | `onChange`, `onInput`, `onFocus`, `onBlur`, `onSubmit` |
| 드래그 | `onDragStart`, `onDragOver`, `onDrop` |
| 터치 | `onTouchStart`, `onTouchMove`, `onTouchEnd` |
| 미디어 | `onPlay`, `onPause`, `onEnded`, `onTimeUpdate`, `onVolumeChange` |
| 생명주기 | `onMount`, `onUnmount` |

```orv
@button onClick={() -> @out "클릭"} "Click me"
@input onKeyDown={(e) -> if e.key == "Enter" : submit()}
```

### 10.6 클래스/스타일 바인딩

`class`는 문자열 또는 표현식으로, `style`은 오브젝트로 지정한다. sig와 결합하면 동적 스타일이 가능하다.

```orv
@div { class="card card-primary" }
@button { class={active ? "btn btn-active" : "btn"} }
@div { style={ backgroundColor: "#1a1a2e", padding: "16px" } }

let sig progress: int = 0
@div { style={ width: "{progress}%" } }
```

### 10.7 @design 토큰

`@design` 도메인으로 디자인 토큰을 선언하면 컴파일러가 CSS 변수 / 번들 CSS로 emit한다. 별도의 CSS 파일이 필요 없다. `@colors`, `@spacing`, `@typography`, `@breakpoints` 하위 도메인을 지원한다.

```orv
@design {
  @colors { primary: "#1a1a2e", accent: "#0f3460", text: "#e4e4e4" }
  @spacing { sm: "8px", md: "16px", lg: "24px" }
  @typography { fontFamily: "Inter, sans-serif" }
  @breakpoints { mobile: "640px", tablet: "768px", desktop: "1024px" }
}

// 사용 -- 경로로 참조
@div { style={ backgroundColor: @design.colors.primary, padding: @design.spacing.md } }
```

### 10.8 반응형 UI (sig + DOM)

sig 변수를 `@html` 도메인 내에서 참조하면 DOM patch가 자동 적용된다. 값 변경 시 해당 노드만 갱신되며, 전체 재렌더는 발생하지 않는다. 조건부 렌더링(`if`)과 리스트 렌더링(`for`)도 sig와 결합하여 반응형으로 동작한다.

```orv
pub define Counter() -> @html {
  @body {
    let sig count: int = 0
    @p "현재 값: {count}"                // count 변경 시 이 텍스트만 patch
    @button onClick={count += 1} "+"
    @button onClick={count -= 1} "-"
  }
}
```

### 10.9 @this 참조

이벤트 핸들러 내에서 `@this`로 현재 DOM 노드를 참조한다.

```orv
@button onClick={@this.remove()} "나를 제거"
@input onFocus={@this.select()} value="전체 선택됨"
```

ref 패턴은 `onMount`로 DOM 핸들을 확보한다.

```orv
let sig input_ref: Element? = void
@input onMount={(el) -> input_ref = el}
@button onClick={input_ref?.focus()} "포커스"
```

### 10.10 브라우저 API

orv는 `window`, `document`, `navigator`, `location`, `history` 등 브라우저 전역 객체에 직접 접근할 수 있다.

| API | 예시 |
|-----|------|
| window | `window.scrollTo(0, 0)`, `window.innerWidth` |
| document | `document.querySelector("#main")`, `document.title = "..."` |
| navigator | `navigator.clipboard.writeText(...)`, `navigator.geolocation.getCurrentPosition()` |
| SPA 라우팅 | `navigate("/about")` (pushState + 라우터 연동) |
| Storage | `localStorage.set("token", "...")`, `sessionStorage.get("draft")` |

객체 저장 시 자동 직렬화된다. `Notification`, `IntersectionObserver`, `ResizeObserver`, `requestAnimationFrame`, `setTimeout`, `setInterval`, `MediaRecorder` 등도 네이티브로 지원한다.

상세 예제: `fixtures/plan/04-web.orv`

### 10.11 @gpu (WebGPU / WebGL2 / SIMD)

`@canvas`의 2D/WebGL 드로잉만으로는 Figma, Photoshop, 3D 게임, GPU 필터 같은 워크로드를 감당할 수 없다. `@gpu` 도메인은 WebGPU 컴퓨트/렌더 파이프라인과 WASM SIMD를 일급(first-class) 프리미티브로 제공한다.

**기본 — 파일 참조 (권장):** 셰이더는 `.wgsl` 파일로 분리하며, 컴파일타임에 검증/번들된다.

```orv
let blur = @gpu.compute file="shaders/blur.wgsl" workgroup=(16, 16, 1)

let out = await blur.dispatch {
  input: inputTexture
  uniforms: { radius: 4.0 }
  size: (width, height, 1)
}
```

**렌더 파이프라인:**

```orv
let scene = @gpu.render {
  vertex: file="shaders/mesh.vert.wgsl"
  fragment: file="shaders/mesh.frag.wgsl"
  targets: [{ format: "rgba8unorm" }]
}

@canvas onMount={(el) -> {
  let ctx = @gpu.context(el)
  loop {
    ctx.begin()
    scene.draw(vertexBuffer, indexBuffer)
    ctx.present()
    await @frame.next()   // requestAnimationFrame 동기화
  }
}}
```

**인라인 escape hatch:** 동적으로 생성되는 셰이더나 한 줄짜리 테스트 셰이더는 `wgsl """..."""` 블록을 사용한다. 권장되지 않는다.

```orv
let quick = @gpu.compute wgsl=""" @compute @workgroup_size(1) fn main() {} """
```

**SIMD / WASM 벡터 타입:**

```orv
// 128비트 SIMD 레지스터
let a: f32x4 = f32x4(1.0, 2.0, 3.0, 4.0)
let b: f32x4 = f32x4.load(pixelBuffer, offset=0)
let sum: f32x4 = a + b
sum.store(pixelBuffer, offset=0)
```

**폴백:** WebGPU 미지원 브라우저는 WebGL2 경로로, SIMD 미지원 런타임은 스칼라 루프로 자동 폴백된다. `@hint gpu=webgl2|webgpu`로 강제 지정 가능.

### 10.12 @media (코덱 / 스트리밍 / Worklet)

미디어 재생/인코딩은 `@video`/`@audio` HTML 태그만으로는 부족하다. `@media` 도메인은 코덱, 어댑티브 스트리밍, Audio Worklet, Media Source Extensions(MSE), Screen/Display Capture를 포괄한다.

```orv
// 어댑티브 스트리밍 서빙 (서버)
@media.stream /watch/:id {
  let asset = await loadAsset(@param.id)
  @manifest hls variants=[
    { bitrate: 400k, resolution: "480p" }
    { bitrate: 1500k, resolution: "720p" }
    { bitrate: 5000k, resolution: "1080p" }
  ]
  @segment duration=4s source={asset.file}
}

// 클라이언트 재생
let player = @media.player src="/watch/abc123" {
  @on qualitychange { @out "품질 전환: {@packet.variant}" }
  @on buffering { @out "버퍼링 {@packet.level}%" }
}
@player.play()
```

**코덱 파이프라인:** 서버 트랜스코딩은 `@media.pipeline`으로 선언하고, 컴파일러가 가능한 경우 하드웨어 가속 인코더(NVENC/QuickSync/VideoToolbox) 바인딩을 선택한다.

```orv
@job transcode (upload: Blob) -> {
  @media.pipeline {
    @decode auto
    @scale 1920 1080
    @encode h264 bitrate=5000k preset=fast
    @package hls segment=4s
    @store "cdn://videos/{upload.id}/"
  }
}
```

**Audio Worklet:**

```orv
@media.audio.worklet "noise-gate" {
  @process (input, output) -> {
    for (sample, i) in input {
      output[i] = abs(sample) > 0.01 ? sample : 0.0
    }
  }
}
```

**Screen Capture:** `@media.screen()` — 화면 공유. `@media.camera()` — 카메라. 둘 다 `MediaStream` 반환으로 `@webrtc` 피어에 그대로 연결 가능.

### 10.13 @offline (ServiceWorker / Cache / IndexedDB)

오프라인 지원은 PWA의 필수 요건이다. `@offline` 도메인은 ServiceWorker 생성, 캐시 전략, 백그라운드 동기화, IndexedDB 바인딩을 하나로 묶는다.

```orv
@offline {
  @cache "assets-v1" strategy=cache-first {
    "/assets/*"
    "/fonts/*"
  }
  @cache "api-v1" strategy=network-first ttl=60s {
    "/api/posts"
    "/api/users/me"
  }

  @sync "outbox" {
    // 네트워크 복구 시 자동 재시도
    for req in await outbox.pending() {
      await api.retry(req)
    }
  }
}

// 오프라인 저장소 — IndexedDB 자동 바인딩
let local: @offline.store<Post> = @offline.store("posts")
await local.put(post.id, post)
let cached = await local.get(postId)
```

### 10.14 @push (Web Push / FCM / APNs)

Push 알림은 플랫폼별 엔드포인트 차이를 추상화한다.

```orv
// 서버측 구독 관리
@route POST /api/push/subscribe {
  @Auth
  let sub: @push.Subscription = @body
  await @db.create PushSubscription %data={ userId: @context.payload.id, ...sub }
  @respond 200 {}
}

// 발송
await @push.send {
  to: subscriberId
  title: "새 메시지"
  body: message.text
  icon: "/icon.png"
  data: { url: "/messages/{message.id}" }
}

// 클라이언트측 권한 요청 + 토큰 등록
let granted = await @push.request()
if granted {
  let sub = await @push.subscribe(vapid=@env.VAPID_PUBLIC)
  await api.fetch("/api/push/subscribe", method="POST", body=sub)
}
```

### 10.15 @textbuffer (rope / piece-table)

대용량 텍스트(코드 에디터, 문서 편집기)에서 `string` 타입의 UTF-8 단순 버퍼는 편집 성능이 O(N)이다. `@textbuffer`는 rope 또는 piece-table 기반으로 O(log N) 삽입/삭제를 제공하며, CRDT(§11.20)와 직접 연동된다.

```orv
let buf: @textbuffer = @textbuffer()
buf.insert(0, "Hello, World!")
buf.delete(5, 7)
@out buf.slice(0, 5)    // "Hello"

// 대용량 파일 스트리밍 로드
let buf2: @textbuffer = await @textbuffer.load("./huge.log")
let lines = buf2.lines(from=1000, to=1100)   // O(log N)로 임의 줄 접근
```

**CRDT 통합:** `@textbuffer.shared(doc=crdtDoc)`로 생성하면 편집이 자동으로 CRDT 오퍼레이션으로 인코딩된다 (§11.20).

---

## 11. 서버 도메인

### 11.1 @server / @listen

`@server` 도메인으로 HTTP 서버를 선언하고, `@listen`으로 포트를 지정한다.

```orv
@server {
  @listen 8080
  @out "서버 시작: 8080"

  @route GET / {
    @respond 200 { message: "Hello, World!" }
  }
}
```

### 11.2 @route (HTTP 메서드)

`@route METHOD /path { ... }` 형태로 라우트를 선언한다. `GET`, `POST`, `PUT`, `DELETE`를 지원하며, `*`는 와일드카드이다.

```orv
@route GET /api/users { ... }
@route POST /api/users { ... }
@route PUT /api/users/:id { ... }
@route DELETE /api/users/:id { ... }
@route GET * { @respond 404 { error: "Not Found" } }
```

경로 매개변수는 `:param` 형태로 선언한다.

**전송 프로토콜:** `@route`는 기본적으로 QUIC(HTTP/3)로 통신한다. 같은 프로젝트 내 서비스 간 통신과 `let` 바인딩 RPC(§11.10)는 항상 QUIC를 사용한다. 외부 클라이언트가 HTTP/3를 지원하지 않을 경우 HTTP/2로 자동 폴백한다.

### 11.3 요청/응답 도메인

| 도메인 | 설명 |
|--------|------|
| `@param` | 경로 매개변수 (`@param.id`, `@param.userId`) |
| `@query` | 쿼리 매개변수 (`@query.page`, `@query.q`) |
| `@header` | 요청 헤더 (`@header.Authorization`, `@header["Content-Type"]`) |
| `@body` | 파싱된 요청 바디 |
| `@request` | 요청 객체 (`.method`, `.path`, `.ip`) |
| `@response` | 응답 객체 (`.status`, `.headers`, `.duration`) |
| `@env` | 환경 변수 (`@env.DATABASE_URL`, `@env.PORT`) |

```orv
@route GET /users/:userId/posts/:postId {
  let userId: int = @param.userId as int
  let postId: int = @param.postId as int
  let q: string = @query.q ?? ""
  let auth: string? = @header.Authorization
  @out @request.method   // GET
}
```

### 11.4 @respond

`@respond` 도메인으로 HTTP 응답을 반환한다. `@respond` 호출 후 코드 실행은 즉시 종료된다 (`return`처럼 동작).

```orv
@respond 200 { status: "ok", data: users }
@respond 201 { user: newUser }
@respond 204 {}
@respond 404 { error: "Not found" }
```

### 11.5 @serve (정적 파일)

정적 디렉토리, 단일 파일, 또는 도메인(페이지)을 서빙한다.

```orv
@route GET /assets { @serve ./public }
@route GET /favicon.ico { @serve ./public/favicon.ico }
@route GET / { @serve @HomePage }
```

### 11.6 미들웨어 (@before, @after, @next, @context)

미들웨어는 도메인으로 정의한다.

- `@before`: 라우트 핸들러 실행 전에 확장
- `@after`: 라우트 핸들러 실행 후에 확장
- `@next`: 다음 핸들러로 진행 (데이터 전달 가능)
- `@context`: 미들웨어에서 `@next`로 전달한 데이터에 접근

```orv
define Auth() -> @before {
  let bearer: string? = @header.Authorization
  when bearer {
    void -> @respond 401 { error: "unauthorized" }
    _ -> {
      let token = bearer[7:]
      let payload = jwt.verify(token, @env.SECRET)
      @next {payload}
    }
  }
}

define AccessLog() -> @after {
  @out "[{@request.method}] {@request.path} -> {@response.status} ({@response.duration}ms)"
  @next
}
```

인자가 없는 미들웨어 도메인은 `@Auth`처럼 괄호 없이 호출한다.

미들웨어 사용:

```orv
@route GET /api/me {
  @Auth
  let user = @context.payload    // @next {payload}로 전달된 데이터
  @respond 200 { user: user }
}
```

### 11.7 라우트 그룹

경로 접두사와 공통 미들웨어를 공유하는 라우트 그룹을 선언할 수 있다.

```orv
@server {
  @listen 8080

  // 전역 미들웨어
  @Cors
  @AccessLog

  // 라우트 그룹
  @route /admin {
    @Auth
    @RateLimit max=30

    @route GET /users { ... }
    @route DELETE /users/:id { ... }
  }
}
```

### 11.8 @db 도메인 (orv-db)

orv-db는 관계형 + 문서형 하이브리드 자체 DB 시스템이며, 기본값이자 권장 저장소이다. 컴파일러 옵티마이저와 연동하여 컴파일타임 쿼리를 최적화하고, IO 성능을 최적화하며, 오버헤드를 최소화한다. PostgreSQL, MongoDB, MySQL 같은 외부 DB는 기본 경로로 전제하지 않지만, 기존 자산 이전과 상호운용을 위해 제한적 어댑터를 제공한다.

#### CRUD 기본

```orv
// Create
let newUser: User = await @db.create User %data={
  name: "Alice"
  email: "alice@orv.dev"
  age: 25
}

// Read — 단일
let user: User = await @db.find User { @where id=1 }

// Read — 복수
let users: User[] = await @db.find User {
  @where age > 18
  @order age=desc
  @skip 0
  @limit 20
}

// Update
await @db.update User {
  @where id=1
  %data={ name: "Alice Updated" }
}

// 원자적 증감
await @db.update Post {
  @where id=postId
  %inc={ likes: 1 }
}

// Delete
await @db.delete User { @where id=1 }
```

#### 쿼리 지시어

| 지시어 | 설명 |
|--------|------|
| `@where` | 필터 조건 |
| `@order` | 정렬 (`field=asc\|desc`) |
| `@skip` | 건너뛸 레코드 수 (페이지네이션) |
| `@limit` | 최대 반환 수 |
| `@field` | 반환할 필드 지정 (프로젝션) |

#### 변이 수정자 (% prefix)

| 수정자 | 설명 |
|--------|------|
| `%data={field: value, ...}` | 설정할 필드와 값 |
| `%inc={field: N, ...}` | 필드를 N만큼 원자적 증감 |

```orv
// 조회수 1 증가 + 마지막 조회 시간 갱신
await @db.update Post {
  @where id=postId
  %inc={ viewCount: 1 }
  %data={ lastViewedAt: now() }
}
```

#### 컴파일타임 + 런타임 하이브리드 쿼리 최적화

컴파일러는 `@db` 호출을 정적 분석하여 다음을 수행한다:
- 존재하지 않는 필드 접근에 대해 컴파일 에러
- 쿼리 플랜 스켈레톤 생성 (인덱스 후보, 조인 순서 초기안)
- 불필요한 필드 로딩 제거 (`@field` 자동 추론)
- 독립적인 쿼리의 자동 병렬화 (§13.4 참조)

정적 결정만으로는 데이터 분포(카디널리티, 히스토그램)를 알 수 없으므로, 런타임 통계를 활용한 재최적화가 결합된다.

- `@db.analyze()` — 통계 수집 (`ANALYZE` 상당)
- `@hint index=<name>` — 수동 인덱스 힌트
- `@hint stats=<strategy>` — `sampled`, `exhaustive`, `none`

런타임 프로파일러는 쿼리 실행 시간과 실제 카디널리티를 축적하며, `orv db tune` 커맨드로 컴파일타임 쿼리 스켈레톤에 되먹임할 수 있다.

#### 인덱스

`@index` 지시어로 명시적 인덱스를 선언한다. 스키마 파일 또는 `@db` 블록 내에서 사용한다.

| 인덱스 타입 | 구문 | 용도 |
|-------------|------|------|
| B-Tree | `@index btree User field=email` | 기본, 동등/범위 검색 |
| Hash | `@index hash Session field=token` | 단순 동등 검색 |
| Full-Text | `@index fulltext Post fields=[title, content] lang=ko` | 텍스트 검색 |
| Vector | `@index vector Doc field=embedding dim=768 metric=cosine` | 의미 검색, 추천 |
| GeoSpatial | `@index geo Place field=location` | 위치 검색 |
| Composite | `@index btree Post fields=[authorId, createdAt]` | 다중 컬럼 |

```orv
// 풀텍스트 검색
let hits = await @db.search Post {
  @match content="사용자 로그인"
  @rank bm25
  @limit 20
}

// 벡터 유사도
let similar = await @db.search Doc {
  @near embedding=queryEmbedding k=10
}
```

#### 트랜잭션

ACID 트랜잭션은 `@db.transaction` 블록으로 선언한다.

```orv
await @db.transaction @hint isolation=serializable {
  let from = await @db.find Account { @where id=fromId }
  let to = await @db.find Account { @where id=toId }

  if from.balance < amount {
    throw { code: "insufficient_funds" }
  }

  await @db.update Account { @where id=fromId; %inc={ balance: -amount } }
  await @db.update Account { @where id=toId; %inc={ balance: amount } }

  audit.log "transfer" { from: fromId, to: toId, amount: amount }
}
```

| 격리 수준 | 용도 |
|----------|------|
| `read-committed` | 기본, 성능 우선 |
| `repeatable-read` | 읽기 일관성 보장 |
| `serializable` | 최고 일관성, 금융 트랜잭션 |
| `snapshot` | MVCC 스냅샷 |

**보장:**
- Write-Ahead Log (WAL) + fsync로 내구성 보장
- 실패 시 자동 롤백
- 중첩 트랜잭션은 savepoint로 변환

#### 샤딩 / 레플리케이션

수평 확장과 고가용성은 `@db` 스키마에서 선언적으로 지정한다.

```orv
@db.schema Post {
  @shard key=authorId count=16         // 샤드 키 + 샤드 수
  @replica count=2 strategy=async       // 읽기 레플리카
  @partition by=createdAt interval=1mo  // 시간 기반 파티셔닝
}
```

컴파일러는 샤드 키를 기반으로 쿼리 라우팅 코드를 자동 생성한다. 크로스-샤드 쿼리는 경고와 함께 scatter-gather로 실행된다.

#### 백업 / 복구

- `orv db backup --target s3://bucket/path` — 포인트-인-타임 백업 (WAL 아카이브)
- `orv db restore --at "2026-04-17T12:00Z"` — 임의 시점 복구 (PITR)
- `@db.connect "postgres://..."` — 외부 DB 어댑터 (PostgreSQL/MySQL/SQLite). 기존 자산 이전/상호운용용이며, 프로덕션 기본 경로로는 비권장

#### 마이그레이션 자동 생성

다른 ORM처럼 마이그레이션 파일을 직접 작성하지 않는다. 컴파일러가 **이전 빌드의 스키마 스냅샷**과 현재 `struct` 선언을 비교하여 마이그레이션을 자동 유도한다.

```
$ orv db plan
migrations/0003_add_user_avatar.orv 미리보기:
  + User.avatar: string?
  + @index btree User field=createdAt

$ orv db apply
Applying 0003_add_user_avatar.orv... OK
```

| 명령 | 동작 |
|------|------|
| `orv db plan` | 현재 struct vs 마지막 적용 스키마 diff → 마이그레이션 dry-run 출력 |
| `orv db apply` | 생성된 마이그레이션 실행 + 스냅샷 업데이트 |
| `orv db rollback` | 최근 마이그레이션 역적용 (가능한 경우) |
| `orv db squash` | 여러 마이그레이션을 하나로 압축 (초기 빌드용) |

**생성 규칙:**
- 필드 추가 → `ADD COLUMN` (기본값 혹은 `?` 필요, 아니면 에러)
- 필드 타입 변경 → 호환 가능(int→long)이면 자동, 아니면 명시적 `@migrate from_fn=...` 필요
- 필드 제거 → 경고와 함께 `DROP COLUMN` 생성, 데이터 백업 옵션 제공
- 인덱스 추가/제거 → 자동
- 샤드 키 변경 → 수동 재구성 스크립트 필요 (컴파일러가 템플릿 생성)

마이그레이션 파일은 `migrations/NNNN_name.orv`로 저장되며, `orv.lock`과 함께 커밋한다. 생성된 파일은 자유롭게 수정 가능하며, 수정 후에는 `orv db verify`로 현재 struct와의 일관성을 검증한다.

### 11.9 @redirect

`@redirect`는 `@route` 내에서 다른 라우트로 리다이렉트하는 도메인이다. `@respond`, `@serve`와 같은 라우트 반환 도메인이며, 호출 후 코드 실행은 즉시 종료된다.

```orv
let loginRoute = @route GET /login {
  @serve @LoginPage
}

@route GET /dashboard {
  @Auth
  let payload = @context.payload
  if payload == void {
    @redirect loginRoute       // 인증 실패 시 로그인 페이지로 리다이렉트
  }
  @serve @DashboardPage
}

// HTTP 상태 코드 지정
@route GET /old-page {
  @redirect 301 "/new-page"   // 301 Moved Permanently
}
```

### 11.10 RPC Facade

라우트를 `let`에 바인딩하면 외부 API에 더해 내부 RPC facade가 추가된다 (하이브리드). 내부 RPC는 바이너리 직렬화를 사용한다. 이때 네트워크에 노출되는 외부 표면적은 기존 `@route` 선언과 배포 설정이 그대로 결정하며, `let` 바인딩은 같은 엔드포인트에 타입 안전한 내부 호출 경로를 하나 더 만드는 것뿐이다. 인증, 미들웨어, 권한 규칙도 동일하게 적용된다. 변수에 할당하지 않으면 외부 API(JSON 직렬화)만 노출된다.

```orv
// 내부 RPC (let 바인딩) -- 바이너리 직렬화
let api = @route /api {
  @route GET /users {
    @respond 200 await @db.find User
  }
}

// 클라이언트에서 사용 -- .fetch()
let users = await api.fetch("/users")

// .fetch() 옵션 — method, body, header
let newUser = await api.fetch("/users", method="POST", body={ name: "Alice" })
let result = await api.fetch("/users/1", method="PUT",
  header={ "X-Custom": "value" },
  body={ name: "Updated" }
)

// 외부 API (변수 미할당) -- JSON 직렬화
@route GET /api/v1/public/health {
  @respond 200 { status: "ok" }
}
```

| | 내부 RPC (let 바인딩) | 외부 API (변수 미할당) |
|---|---|---|
| 직렬화 | 바이너리 | JSON |
| 접근 | `.fetch()`만 가능 | curl, 브라우저 등 |
| 스키마 | 컴파일타임 고정 | 표준 HTTP |

컴파일러는 존재하지 않는 경로나 잘못된 타입에 대해 컴파일 에러를 발생시킨다.

단일 라우트를 `let`에 바인딩한 경우에도 `.fetch()`는 문자열 경로를 사용하며, 경로는 바인딩된 정적 prefix를 기준으로 상대 해석된다. 예: `getUser.fetch("/42")`.

#### 자동 타입 추론 규칙

`api.fetch()`는 제네릭 인자나 명시적 타입 지정 없이 호출한다. 컴파일러는 경로 문자열을 파싱하여 `@route` 선언과 매칭하고, 다음을 전부 추론한다.

| 위치 | 추론 대상 | 근거 |
|------|---------|------|
| 경로 `:param` | 타입 + 필수 여부 | `@param.id as int` 등 바디 내부 변환 |
| `query` | 쿼리 스키마 | `@query.page` 참조 |
| `body` | 바디 struct | `let body: T = @body` 선언 |
| 반환 | 응답 타입 | 각 `@respond` 블록 payload 유니온 |
| 상태 코드 | 유니온 | 라우트 내 모든 `@respond NNN` 수집 |

```orv
let postApi = @route /api/posts {
  @route POST /:id/comments {
    let id: int = @param.id as int
    let body: { content: string } = @body
    let comment = await @db.create Comment %data={ postId: id, ...body }
    @respond 201 comment       // 성공: Comment
    @respond 409 { error: "duplicate" }   // 실패: { error: string }
  }
}

// 호출부 — 타입 주석 없음
let c = await postApi.fetch("/42/comments", method="POST", body={ content: "안녕" })
// 타입: Comment | { error: string } (응답 유니온)
// 컴파일러는 @respond status에 따라 narrowing 제공

when c {
  Comment.is -> @out "작성: {c.id}"
  _ -> @out "충돌: {c.error}"
}
```

**경로 매칭:** 리터럴 문자열 템플릿도 컴파일타임에 해석된다. `api.fetch("/posts/{id}/comments")`에서 `id`의 타입이 라우트 `:id` 바디 변환(`@param.id as int`)과 일치하지 않으면 컴파일 에러이다.

**쿼리/메서드 축약:** 자주 쓰는 메서드는 호출 편의 메서드로 노출된다.

```orv
let c = await postApi.posts[42].comments.create({ content: "안녕" })
// → POST /api/posts/42/comments 로 변환
```

이 경로 DSL은 `@route` 선언 구조를 따라 타입이 확정되며, 존재하지 않는 노드 접근은 컴파일 에러이다.

### 11.11 WebSocket (@ws)

`@ws` 도메인으로 WebSocket 엔드포인트를 선언한다. HTTP `@route`와 별도의 실시간 통신 도메인이다.

```orv
@ws /chat {
  @connect { @emit welcome to=@socket.id { msg: "Hello!" } }
  @on message { @emit message @packet }                          // broadcast
  @on join-room {
    @socket.join(@packet.room)
    @emit room-update in=@packet.room { user: @socket.id, action: "joined" }
  }
  @disconnect { @emit user-left @socket.id }
}
```

**@emit 전송 방식:** `@emit channel data` (broadcast), `@emit channel to=id data` (특정 소켓), `@emit channel in="room" data` (룸 내 전체).

**@ws 빌트인:** `@socket.id` (소켓 식별자), `@socket.join(room)`, `@socket.leave(room)`, `@packet` (수신 데이터).

**클라이언트 facade:** `@ws`를 `let`에 바인딩하면 외부 표준 WebSocket에 더해 내부 바이너리 RPC facade가 추가된다 (§11.10과 동일 원칙). 컴파일타임에 채널 스키마를 검증한다. 변수에 할당하지 않으면 표준 WebSocket + JSON 페이로드만 노출된다.

```orv
let chat = @ws /chat { ... }
let myWS = await @chat.connect
@myWS.emit message { text: "안녕" }
@myWS.on message { @out "수신: {@packet}" }
await @myWS.close
```

상세 예제: `fixtures/plan/05-server.orv`

### 11.12 WebTransport (@wt)

`@wt` 도메인으로 WebTransport 엔드포인트를 선언한다. QUIC 기반 HTTP/3 전송으로, 브라우저에서 접근 가능하다. 스트림(stream)과 데이터그램(datagram) 두 가지 채널을 제공한다.

- **stream** — 신뢰성 있는 순서 보장 전송. `bidi` (양방향) 또는 `uni` (서버→클라이언트 단방향).
- **datagram** — 비신뢰성 비순서 전송. 저지연 실시간 데이터에 적합.

```orv
@wt /game {
  @connect { @out "[WT] {@session.id} connected" }

  @stream bidi inventory {
    @recv {
      let result = await updateInventory(@session.id, @data)
      @send { updated: result }
    }
  }

  @stream uni replay {
    for event in await getReplayEvents() { @send event }
  }

  @datagram position {
    @recv { broadcastPosition(@session.id, @data) }
  }

  @disconnect { @out "[WT] {@session.id} disconnected" }
}
```

**@wt 빌트인:** `@session.id` (세션 식별자), `@stream bidi <name>` / `@stream uni <name>` (스트림 선언), `@datagram <name>` (데이터그램 채널 선언), `@send data` (전송), `@recv { handler }` (수신 핸들러), `@data` (수신된 데이터).

**클라이언트 facade:** `let`에 바인딩하면 바이너리 프로토콜 적용 (§11.10 참조).

```orv
let game = @wt /game { ... }
let session = await game.connect()

// 스트림 — 신뢰 전송
let inv = await @session.stream.inventory
@inv.send { action: "equip", item: "sword" }
@inv.recv { @out "업데이트: {@data.updated}" }

// 데이터그램 — 저지연
@session.datagram.position.send { x: 100.0, y: 200.0 }

await @session.close
```

**컴파일타임 검증:** 존재하지 않는 스트림 접근, uni 스트림에 `send` 호출, 스키마 불일치는 컴파일 에러이다.

### 11.13 WebRTC (@webrtc)

`@webrtc` 도메인은 P2P 통신을 위한 시그널링 서버를 선언한다. 실제 미디어/데이터 전송은 피어 간 직접 이루어지며, 서버는 offer/answer/ICE candidate 교환만 중계한다.

```orv
@webrtc /call {
  @signal {
    @on offer {
      @signal to=@data.target { type: "offer", from: @peer.id, sdp: @data.sdp }
    }
    @on answer {
      @signal to=@data.target { type: "answer", from: @peer.id, sdp: @data.sdp }
    }
    @on ice-candidate {
      @signal to=@data.target { type: "ice-candidate", from: @peer.id, candidate: @data.candidate }
    }
  }

  @connect {
    @signal to=@peer.id { type: "peers", peers: getConnectedPeers() }
  }
  @disconnect {
    @signal { type: "peer-left", peer: @peer.id }
  }
}
```

**@webrtc 빌트인:** `@peer.id` (피어 식별자), `@signal { ... }` (시그널링 블록), `@signal to=id data` (특정 피어에 시그널 전송), `@signal data` (broadcast), `@data` (수신된 시그널 데이터).

**클라이언트 facade:** `let` 바인딩 시 외부 표준 시그널링에 더해 내부 바이너리 시그널링 facade가 추가된다. 미디어/데이터는 항상 WebRTC 표준을 사용한다. 미할당 시 표준 WebRTC 시그널링만 노출된다.

```orv
let callSignal = @webrtc /call { ... }
let signaling = await callSignal.connect()

let pc = @peer.create()
let localStream = await @media.getUserMedia { video: true, audio: true }
for track in localStream.tracks { @pc.addTrack track }

let offer = await @pc.createOffer()
await @pc.setLocalDescription(offer)
@signaling.signal { type: "offer", target: targetPeerId, sdp: offer.sdp }

@signaling.on answer { await @pc.setRemoteDescription(@data.sdp) }
@pc.onTrack { @out "원격 트랙: {@track.kind}" }

let dc = @pc.createDataChannel "chat"
@dc.send { text: "P2P 메시지" }
@dc.onMessage { @out "수신: {@data.text}" }
```

### 11.14 전송 프로토콜 비교

| 도메인 | 전송 계층 | 방향 | 신뢰성 | 주 용도 |
|--------|----------|------|--------|---------|
| `@route` | QUIC(H3) 기본, H2 폴백 | 요청-응답 | 신뢰 | REST API, 페이지 서빙 |
| `@ws` | WebSocket | 양방향 메시지 | 신뢰 | 실시간 채팅, 알림 |
| `@wt` | WebTransport (H3) | 양방향 스트림/데이터그램 | 선택 | 게임, 미디어, 저지연 |
| `@webrtc` | WebRTC | P2P | 선택 | 미디어 통화, P2P 데이터 |

**@route 프로토콜:** QUIC(HTTP/3) 기본. 외부 클라이언트가 H3을 지원하지 않으면 HTTP/2로 자동 폴백한다.

**let 바인딩 규칙 (모든 도메인 공통):** 외부 표준 프로토콜 노출은 항상 기본이다. `let binding = @protocol ...`은 동일 엔드포인트에 내부 바이너리 facade를 추가하지만, 외부 노출 여부나 기존 인증/미들웨어 규칙을 바꾸지는 않는다 (§11.10 참조).

상세 예제: `fixtures/plan/05-server.orv`

### 11.15 @upload / @storage

대용량 파일(Drive, YouTube 업로드, 메일 첨부)은 chunked + resumable 업로드가 필수이다. `@upload` 도메인은 분할, 재개, 병합, 진행률을 선언적으로 처리한다.

```orv
@upload POST /api/files chunked size=5mb resumable {
  @Auth
  @on chunk {
    // 청크 수신 시마다 호출 — 순서 무관, 중복 가능
    await @storage.putChunk(@upload.id, @chunk.index, @chunk.data)
  }
  @on complete {
    // 모든 청크 수신 완료 시 병합
    let file = await @storage.merge(@upload.id, target="uploads/{@upload.id}")
    await @db.create FileMeta %data={
      userId: @context.payload.id
      size: file.size
      contentType: file.contentType
    }
    @respond 201 { fileId: @upload.id }
  }
}
```

**`@storage` 추상화:** 로컬 디스크, S3 호환 오브젝트 스토리지, GCS, Azure Blob을 공통 인터페이스로 제공한다.

```orv
@storage.backend s3 {
  endpoint: @env.S3_ENDPOINT
  bucket: @env.S3_BUCKET
  region: "ap-northeast-2"
  credentials: vault.get("s3/creds")
}

await @storage.put("avatars/{userId}.png", blob, contentType="image/png")
let url: string = await @storage.signedUrl("videos/{id}.mp4", ttl=3600)
```

**Range 스트리밍 다운로드:** `@serve`는 확장자 기반 단순 정적 서빙이다. `@storage.stream(path)`는 HTTP Range 헤더를 자동 처리하여 비디오 seek, 재개 다운로드를 지원한다.

### 11.16 @net (raw TCP/UDP / TUN)

HTTP 레벨로는 VPN, 커스텀 프로토콜 프록시, 게임 서버의 저수준 제어가 불가능하다. `@net` 도메인은 raw 소켓과 OS 가상 인터페이스를 노출한다. **FFI 영역**이므로 `unsafe` 컨텍스트가 필요하다.

```orv
// TCP 서버
@net.tcp /listener port=9000 {
  @on connection {
    let conn = @connection
    spawn {
      for await chunk in conn.read() {
        await conn.write(process(chunk))
      }
    }
  }
}

// UDP
@net.udp port=5555 {
  @on packet {
    let response = handlePacket(@data)
    @send to=@packet.from response
  }
}

// TUN 가상 인터페이스 (VPN)
let tun = await @net.tun.create(name="orv0", ipv4="10.8.0.1/24")
for await packet in tun.read() {
  let encrypted = crypto.encrypt(packet, sessionKey)
  await tunnelChannel.send(encrypted)
}
```

### 11.17 @mail (SMTP / IMAP 서버/클라이언트)

§8.3의 `smtp`/`imap` 스탠다드 라이브러리가 클라이언트 프로토콜이라면, `@mail` 도메인은 **메일 서버**를 구축하는 도메인이다. Mail 앱의 수신 서버, 스팸 필터, 포워딩 규칙을 처리한다.

```orv
@mail.smtp port=25 {
  @on message {
    let verified = await @mail.verify.dkim(@message) && @mail.verify.spf(@message)
    if !verified {
      @reject 550 "Authentication failed"
    }
    if await spamScore(@message) > 0.8 {
      @store "spam"
    } else {
      @store "inbox"
    }
  }
}

@mail.imap port=993 tls {
  @Auth
  // 클라이언트에 메일박스 API 자동 노출
}
```

### 11.18 @cron / @job

`spawn`(§7.3)은 요청 스코프 태스크이다. 주기 실행, 백그라운드 Job 큐, 재시도 정책은 별도 프리미티브가 필요하다.

```orv
// Cron — 시간 기반 스케줄
@cron "0 9 * * *" {
  let pending = await @db.find Reminder { @where scheduledAt <= now() }
  for r in pending {
    await @push.send { to: r.userId, title: r.title }
  }
}

// Job 큐 — 비동기 백그라운드 작업
@job transcode priority=high retries=3 backoff=exponential (input: Blob) -> {
  await @media.pipeline {
    @decode auto
    @encode h264
    @store "cdn://videos/"
  }
}

// 큐잉 — 인자 타입이 @job 시그니처로부터 자동 추론
await @job.transcode.enqueue(uploadBlob)
// → 잘못된 타입(string 등) 전달 시 컴파일 에러
```

**타입 안전 큐:** `@job` 본문의 파라미터 시그니처가 `.enqueue()` 호출의 타입 제약이 된다. 명시적 제네릭은 필요 없다. 재시도 결과도 타입으로 추론된다.

```orv
@job settleOrder priority=high retries=5 backoff=exponential (order: Order) -> Receipt {
  // ... 성공 시 Receipt 반환, 실패 시 throw → 자동 재시도
}

// enqueue — 인자 타입 강제
let handle = @job.settleOrder.enqueue(order)
// handle: Job<Receipt> — 대기 상태 조회, 결과 await 가능
let receipt: Receipt = await handle
```

**재시도/백오프 정책:** `retries`, `backoff`, `delay`, `deadline` 속성이 컴파일타임 상수이면 플랜이 AOT로 생성되어 런타임 오버헤드가 없다. 동적 값이 필요하면 `enqueue(input, retries=n)`으로 호출 시 오버라이드 가능하다.

**Job 상태 저장:** `@db`의 내장 `Job` 테이블에 자동 기록된다. Worker 재시작 시에도 in-flight 작업은 중단점부터 재개된다 (`@job` 본문의 `await` 경계 단위 체크포인트).

**Cron 파싱:** `@cron "0 9 * * *"` 표현식은 컴파일타임에 파싱되어 AST가 된다. 잘못된 표현식은 컴파일 에러이다. 배포 시 분산 스케줄러가 여러 인스턴스 중 리더 선출 후 한 번만 실행한다.

### 11.19 @plugin (런타임 확장)

VSCode 확장, Notion 블록 플러그인 같은 **런타임 동적 로딩**이 필요한 경우에만 사용한다. 정적 확장은 `define`이 우선이다.

```orv
// 호스트 — 플러그인 API 서피스 정의
@plugin.host "editor" {
  @api onCommand(id: string, handler: Function<Context, void>) -> void
  @api registerLanguage(spec: LanguageSpec) -> void
  @permissions [fs.read, net.fetch]    // 플러그인이 요청 가능한 권한
}

// 플러그인 로딩 — WASM 샌드박스에서 실행
let plugin = await @plugin.load "ext/markdown-preview.wasm"
await plugin.activate()

// 플러그인 쪽 코드
@plugin.entry {
  @on activate {
    @host.editor.onCommand("preview.toggle", (ctx) -> {
      togglePreview(ctx.currentFile)
    })
  }
}
```

**격리:** 각 플러그인은 자체 WASM 인스턴스에서 실행되며, 호스트와는 capability 기반 RPC로만 통신한다. 허용되지 않은 시스템 콜/fs 접근은 런타임에 차단된다.

### 11.20 @sync (CRDT / OT)

실시간 협업 편집(Notion, Google Docs, Figma, 공유 화이트보드)의 핵심은 충돌 해소 알고리즘이다. orv는 CRDT를 **언어 프리미티브**로 제공하되, 별도의 타입 계층을 추가하지 않는다. `@sync`는 **struct 수식자**로 동작하여 필드 타입을 CRDT 의미론으로 승격시키되, 기본값은 의도 보존을 우선하는 보수적 규칙을 따른다.

```orv
// 일반 struct에 @sync 수식자만 붙이면 CRDT 문서가 된다
@sync struct Drawing {
  strokes: Stroke[]                     // 자동으로 순서 있는 CRDT 리스트로 승격
  cursor: Map<string, Point>            // 자동으로 LWW-Map으로 승격
  title: string                         // 기본적으로 LWW 문자열
  version: int @crdt counter            // 수식자로 알고리즘 명시 가능
}

// 서버 — 문서 호스팅
@sync /doc/:id {
  @Auth
  let doc = await @sync.open(Drawing, @param.id)
  @session join=doc
}

// 클라이언트 — 자동 동기화 (struct 사용감 그대로)
let doc = await @sync.connect(Drawing, "/doc/abc")
doc.strokes.push(newStroke)        // 자동 broadcast, 자동 머지
doc.cursor.set(userId, { x, y })
doc.title = "새 제목"                // LWW 문자열 업데이트

// 구독
@sync.observe(doc.strokes) {
  redraw(doc.strokes)
}
```

**자동 승격 규칙 (기본값):**

| 필드 타입 | 승격된 CRDT | 알고리즘 |
|----------|-----------|---------|
| `T[]` | 순서 있는 리스트 | RGA / YATA |
| `Map<K, V>` | 키-값 맵 | LWW / OR-Map |
| `Set<T>` | 집합 | OR-Set |
| `string` | LWW 문자열 | LWW |
| `int` / `float` | LWW 스칼라 | LWW |
| 중첩 struct | 재귀적 CRDT | 필드별 규칙 재적용 |

**수식자 덮어쓰기:** 긴 본문처럼 협업 편집 의미가 필요한 필드는 필드 레벨 `@crdt` 수식자로 알고리즘을 명시한다.

```orv
@sync struct Counter {
  total: int @crdt counter=pn          // PN-Counter 명시
  tree: Block[] @crdt movetree          // Move-op CRDT 트리
  title: string @crdt text              // 기본값(LWW) 대신 협업 텍스트로 승격
}
```

**저장/복구:** CRDT 상태는 `@db`에 오퍼레이션 로그로 저장되며, 스냅샷은 주기적으로 압축된다. `@wt` 스트림을 통해 변경이 실시간 전파된다.

**오프라인:** 클라이언트는 로컬 `@offline.store`에 오퍼레이션을 버퍼링하고, 연결 복구 시 자동 플러시한다.

### 11.21 @observability (트레이싱 / 메트릭 / 구조화 로그)

프로덕션 운영의 기본 요구인 분산 트레이싱, RED 메트릭, 구조화 로그를 명시적 계측 코드를 최소화한 상태로 수집한다. 컴파일러는 모든 `@route`, `@db`, `@job`, `@ws`, `@wt`, `@sync` 호출 경계에 관측 코드를 합성하고, 선택되지 않은 항목은 빌드 프로필에 따라 제거한다.

```orv
@observability {
  service: "superapp"
  exporter: otlp endpoint=@env.OTEL_ENDPOINT
  sample: ratio=0.1 @when error=always   // 에러 발생 시 100%, 정상 10%
  metrics: [latency, rps, error_rate, db_query_time]
  logs: structured level=info
}
```

**자동 계측 대상:**

| 도메인 | 수집 항목 |
|--------|---------|
| `@route` | 메서드, 경로, 상태, 지연시간, 사용자 ID |
| `@db` | 쿼리 플랜, 실행 시간, 결과 행 수 |
| `@job` | 큐 대기시간, 실행 시간, 재시도 횟수 |
| `@ws`/`@wt` | 연결 수, 메시지 수, 지연, 재연결 |
| `@sync` | 오퍼레이션 수, 충돌 횟수, 머지 시간 |

**전파:** `@route` 진입 시 trace context (W3C Trace Context) 자동 추출/생성. 내부 `api.fetch`/`@job.enqueue`/`@db` 호출에 자동 전파. 수동 span 작성 없이 주요 풀스택 분산 추적 흐름이 연결된다.

**커스텀 이벤트:** 애플리케이션 레벨 이벤트는 기존 `audit.log`(§8.3)와 `@out`이 구조화 로그로 자동 승격된다. 추가 API 없음.

**기본값 최소 오버헤드 원칙:** `@observability` 블록이 없으면 관련 관측 코드는 프로덕션 번들에 생성하지 않는다. 개발 빌드에서는 로컬 콘솔에 폴백 출력한다.

상세 예제: `fixtures/plan/08-superapp-simulation.orv`

---

## 12. 컴파일러 지시어

### 12.1 @hint 디렉티브

모든 최적화는 자동이지만, `@hint`로 컴파일러 결정을 덮어쓸 수 있다.

| @hint | 대상 | 값 |
|-------|------|----|
| `render=` | 페이지(`@html`) | `ssr`, `csr`, `ssg` |
| `protocol=` | 라우트 | `json`, `binary`, `hybrid` |
| `cache=` | fetch / 쿼리 | `never`, `immutable`, `{N}s` |
| `prefetch=` | fetch | `never`, `eager` |
| `chunk=` | 라우트 / 모듈 | `separate`, `inline` |
| `batch=` | fetch (루프 내) | `never` |
| `keep` | import / 변수 | tree-shake 방지 |

```orv
pub define AlwaysSSR() -> @html @hint render=ssr { ... }       // 렌더링 전략 강제
let jsonForced = @route GET /api/compat @hint protocol=json {} // 프로토콜 강제
let live = await getUser.fetch("/1") @hint cache=never
@hint keep
let analytics = loadAnalytics()                                // tree-shake 방지
@route GET /admin @hint chunk=separate { ... }                 // 청킹 강제
```

### 12.2 Void 스코프 auto-output 규칙

void 스코프(파일 최상위, 함수 본문, 도메인 본문)에서 문자열 리터럴의 처리 규칙은 다음과 같다.

- **마지막 표현식**: 반환값(`return`)으로 처리
- **마지막이 아닌 표현식**: `@out`에 얹혀져 자동 출력

```orv
// 파일 최상위
"Hello, World!"    // == @out "Hello, World!"

// 함수 내부
function showAndReturn(a: int, b: int): string -> {
  "{a}"           // @out -- 출력
  "{b}"           // @out -- 출력
  "{a + b}"       // return -- 마지막이므로 반환
}
```

---

## 13. 최적화 모델

orv 컴파일러는 "프로젝트 특화" 최적화를 수행한다. 핵심 원칙은 다음과 같다.

1. 컴파일 타임에 가능한 모든 결정을 확정한다.
2. 사용하지 않는 기능은 번들에 포함하지 않는다 (zero-overhead).
3. 정적 산출만으로 충분한 타깃에는 별도 런타임 계층을 포함하지 않는다 (zero-runtime).
4. 기본은 자동이며, `@hint`(§12.1)로 오버라이드할 수 있다.

### 13.1 DCE (Dead Code Elimination)

엔트리 포인트에서 도달 가능한(reachable) 코드만 번들에 포함한다. 나머지는 전부 제거된다.

**도메인 레벨 DCE:** `@html` 관련 도메인을 사용하지 않으면 UI 런타임 전체가 번들에서 제거된다. CLI 도구라면 DOM 바인딩, HTTP 서버, 라우터 코드가 전부 제거되어 순수 바이너리만 생성된다.

**기능 레벨 DCE:**

| 미사용 기능 | 제거 대상 |
|------------|-----------|
| sig | 리액티비티 추적기 |
| when | 패턴 매칭 코드젠 |
| async/await | 비동기 런타임 |
| try/catch | 예외 처리 인프라 |

### 13.2 sig 도메인 인식 업데이트

sig를 어디서 사용하느냐에 따라 번들에 포함되는 런타임이 달라진다 (§3.3 참조).

- `@out`에서만 사용 -- 콘솔 replace 코드만 포함, DOM patch 코드 제거
- `@html`에서만 사용 -- DOM patch 코드만 포함, 콘솔 코드 제거
- sig를 전혀 사용하지 않음 -- 리액티비티 추적기 자체 제거

### 13.3 Auto-batching

루프 내 동일 fetch 호출을 컴파일러가 단일 batch 요청으로 변환한다. 서버 측 배치 핸들러도 자동 생성된다.

```orv
// 개발자가 작성
for id in ids {
  let user = await getUser.fetch("/{id}")
  @li "{user.name}"
}

// 컴파일러가 변환 (내부)
// N번의 HTTP 왕복 -> 1번으로 줄어듦
```

`@hint batch=never`로 비활성화할 수 있다.

### 13.4 Auto-parallelization

같은 스코프 내 독립적인 fetch/쿼리는 자동으로 병렬 실행된다. 의존 관계가 있으면 순차를 유지한다. 혼합 패턴에서는 가능한 부분만 병렬로 실행한다.

```orv
let user = await @db.find User { @where id=id }       // 1. 먼저
let posts = await @db.find Post { @where author=user.id }  // 2-a. user 의존
let followers = await @db.find Follower { @where user=user.id }  // 2-b. user 의존 (posts와 독립)
let trending = await @db.find Trending                 // 2-c. 독립
// 실행: user -> (posts || followers || trending)
```

### 13.5 렌더링 전략 추론

컴파일러가 코드 패턴을 분석하여 렌더링 전략을 자동 결정한다. SSR, CSR, SSG 키워드가 코드에 등장하지 않는다.

| 패턴 | 판정 | 결과 |
|------|------|------|
| sig 0, await 0 | 정적 생성 | 빌드 시 HTML 확정, JS/WASM 없음 |
| sig 있음, 서버 데이터 없음 | 클라이언트 렌더링 | 빈 HTML 쉘 + WASM 번들 |
| await로 서버 데이터 사용 | 서버 렌더링 | 서버 HTML 생성 + 클라이언트 hydration |

`@hint render=ssr|csr|ssg`로 강제 지정할 수 있다 (§12.1 참조).

### 13.6 번들 분할

단일 `.orv` 파일에 `@server`와 `@html`이 공존하면 자동으로 서버 바이너리와 클라이언트 번들(HTML + JS 브릿지 + WASM + CSS)이 분리 생성된다.

핵심 규칙: 서버 번들에 UI 코드 없음, 클라이언트 번들에 DB/미들웨어 코드 없음, RPC 타입 스키마만 양쪽에 공유. 코드 스플리팅은 라우트 기반으로 자동 분리되며, 공유 도메인은 `chunk-shared`로 추출된다.

### 13.7 프로토콜 최적화

내부 RPC(let 바인딩)는 바이너리 직렬화(필드명 없이 순서 기반 인코딩)를 사용하며, Content-Type과 Content-Length를 컴파일타임에 확정한다. 외부 API(변수 미할당)는 JSON 직렬화와 표준 HTTP를 사용하되, 헤더를 컴파일타임에 바이트 배열로 미리 직렬화한다.

HTTP 메타데이터도 자동 결정된다: `@respond 200 {...}` -- `application/json` 고정, `@serve ./public` -- 확장자 기반 MIME 매핑 빌드타임 생성, `@respond 204 {}` -- body 인코더 제거, `@before`가 없는 라우트 -- 미들웨어 디스패치 건너뜀.

### 13.8 적응형 런타임

컴파일러가 emit하는 경량 런타임은 프로덕션에서 관찰(요청/응답 시간, 캐시 히트율, 쿼리 패턴, 커넥션 풀 사용률)하여 파라미터를 튜닝한다. 코드를 재작성하거나 전략을 근본적으로 바꾸지는 않는다.

자동 조절 예시: 동일 파라미터 재호출 시 인메모리 캐시 활성화, 반복 접근 패턴 감지 시 프리페치, API 응답 시간 증가 시 커넥션 풀 확장 등. 재빌드가 필요한 전환(CSR/SSR 전환)은 리포트로 제안만 한다.

### 13.9 증분 컴파일

전체 프로젝트 분석 기반 최적화는 처음 빌드에서만 비용이 발생해야 한다. 증분 컴파일은 다음 원칙을 따른다.

1. **모듈 단위 캐시** — 각 `.orv` 파일의 AST, HIR, 프로젝트 그래프 기여분을 파일별로 캐싱한다 (`.orv/cache/<hash>/`). salsa 스타일 쿼리 엔진을 사용한다.
2. **시그니처 vs 본문 분리** — `pub define`/`pub struct`/`pub function`의 시그니처 변경만이 의존 파일의 재분석을 유발한다. 본문만 바뀌면 해당 파일만 재생성한다.
3. **프로젝트 그래프의 부분 재계산** — `orv-project`는 파일별 기여분 합집합으로 그래프를 구성하므로, 변경된 파일의 기여분만 재계산한다.
4. **HMR (개발 서버)** — `orv dev`는 변경된 도메인만 교체하고, `sig` 상태는 보존한다. 타입 불일치 시 전체 재로드로 폴백한다.

**캐시 무효화 매트릭스:**

| 변경 종류 | 재계산 범위 |
|----------|-----------|
| 주석/공백 | 없음 (해시 변동 시만) |
| 함수 본문 | 해당 파일만 |
| `pub` 시그니처 | 해당 파일 + 직접 import한 파일 |
| 스키마 필드 추가 | 해당 타입을 사용하는 `@db` 호출의 쿼리 플랜 재생성 |
| `@design` 토큰 | CSS 재추출 |
| `orv.toml` 의존성 | 전체 재빌드 |

대규모 프로젝트(>1M LOC)에서 전체 빌드 20분이 단일 파일 변경 시 1초 이하로 수렴하도록 설계한다.

### 13.10 WASM 타겟 프로파일

orv 컴파일러가 emit하는 WASM은 브라우저/런타임 지원에 따라 복수 프로파일을 발행할 수 있다.

| 프로파일 | 포함 기능 | 용도 |
|---------|----------|------|
| `baseline` | MVP + bulk-memory | 최대 호환 (구형 브라우저) |
| `modern` | + SIMD + Thread + Exception Handling | 데스크톱/최신 모바일 (기본) |
| `performance` | + Memory64 + GC proposal + relaxed SIMD | 그래픽스/편집기/게임 |
| `native` | WASI + 시스템 콜 | 서버 네이티브 실행 |

`orv.toml`의 `[build]`에서 지정한다.

```toml
[build]
wasm.profiles = ["baseline", "modern"]
wasm.default = "modern"
```

컴파일러는 같은 페이지의 `baseline.wasm`과 `modern.wasm`을 동시 emit하고, 런타임에 `WebAssembly.validate`로 feature detection하여 최적 프로파일을 로드한다.

**RC → WASM 매핑:** 기본은 WASM 선형 메모리 위에 직접 RC 카운터를 쌓는다. `performance` 프로파일에서는 WASM GC proposal(`anyref`, `struct`, `array`)에 매핑하여 브라우저 GC와 상호운용한다.

### 13.11 번들 크기 목표의 앱 유형별 분화

"SPA 번들 ≤ 3kb"는 **정적/가벼운 대화형** 페이지에 한정된 목표이다. 실제 슈퍼앱 워크로드는 훨씬 큰 번들이 불가피하며, orv는 앱 유형별 현실적 목표를 제시한다.

| 앱 유형 | 초기 번들 목표 | 비고 |
|---------|--------------|------|
| 정적 랜딩/블로그 (sig 0, await 0) | 0 byte (순수 HTML) | JS/WASM 없음 |
| 가벼운 대화형 (sig 사용) | ≤ 3 KB | 반응형 런타임만 |
| 표준 SPA (SSR + hydration) | ≤ 30 KB 초기 + 라우트별 lazy | React급 기능 풀커버 |
| 그래픽스/미디어 (Figma/Photoshop급) | ≤ 200 KB 초기 쉘 + 스트리밍 로드 | WebGPU 셰이더/코덱은 별도 청크 |
| 게임 (Krunker급) | ≤ 1 MB 초기 + 에셋 스트리밍 | WASM + WebGL/WebGPU + 에셋 |

**공통 원칙:** 초기 번들은 "첫 인터랙션까지 필요한 최소 코드"만 포함하고, 나머지는 라우트 기반 lazy loading 또는 Service Worker 캐싱으로 뒤로 밀린다. DCE, 코드 스플리팅, 프로필 가드는 자동이다.

상세 예제: `fixtures/plan/06-optimization.orv`, `fixtures/plan/08-superapp-simulation.orv`

---

## 부록: 입출력 및 내장 도메인

### I/O 도메인

| 도메인 | 설명 | 예시 |
|--------|------|------|
| `@out` | 표준 출력 | `@out "hello"` |
| `@in` | 표준 입력 | `let input: string = @in "프롬프트:"` |
| `@fs.read` | 파일 읽기 | `let file: File = await @fs.read test.txt` |
| `@fs.write` | 파일 쓰기 | `await @fs.write ./test.txt "내용" utf-8` |
| `@fetch` | HTTP 요청 | `let res: Response = await @fetch GET "https://..."` |
| `@process.run` | 프로세스 실행 | `let r = await @process.run "ls -la"` |

### 내장 함수

| 함수 | 설명 |
|------|------|
| `Type(x)` | 값의 타입 반환 |
| `max(a, b, ...)` | 최대값 |
| `min(a, b, ...)` | 최소값 |
| `abs(x)` | 절대값 |
| `sin(x)`, `cos(x)`, `tan(x)`, `log(x)` | 수학 함수 |
| `now()` | 현재 시각 (`Time` 타입) |
| `today()` | 오늘 날짜 |
| `tomorrow()` | 내일 날짜 |
| `yesterday()` | 어제 날짜 |
| `sleep(ms)` | 비동기 대기 |
| `navigate(path)` | SPA 라우팅 (§10.10) |

### 문자열 메서드

| 메서드 | 설명 |
|--------|------|
| `str[0:5]` | 슬라이싱 |
| `str.length` | 길이 |
| `str.contains(s)` | 포함 여부 |
| `str.toLowerCase()` | 소문자 변환 |
| `str.replace(a, b)` | 치환 |
| `str.join(sep)` | 배열 결합 (배열 메서드) |

---

---

## 14. 테스트 프레임워크

orv는 테스트를 내장 기능으로 제공한다. 별도의 테스트 라이브러리가 필요 없다.

### 14.1 테스트 선언

`test` 키워드로 테스트를 선언한다. 테스트 이름은 문자열로 지정한다.

```orv
test "두 수의 합" {
  let result = add(1, 2)
  assert result == 3
}

test "사용자 생성" {
  let user: User = { name: "Alice", age: 25 }
  assert user.name == "Alice"
  assert user.age == 25
}
```

### 14.2 assert

`assert`는 조건이 `false`이면 테스트를 실패 처리한다.

```orv
assert value == expected
assert list.length > 0
assert User.validate(input)
```

### 14.3 비동기 테스트

비동기 테스트는 `test "이름" async { ... }` 형태로 작성한다.

```orv
test "API 응답 확인" async {
  let res: Response = await @fetch GET "http://localhost:8080/api/health"
  assert res.status == 200
}
```

### 14.4 테스트 실행

```
orv test              # 전체 테스트 실행
orv test src/models   # 특정 디렉토리
orv test --filter "사용자"  # 이름 필터
```

---

## 15. 패키지 매니저

### 15.1 orv.toml — 프로젝트 매니페스트

`orv.toml`은 프로젝트 설정 파일이다. Cargo.toml 스타일을 따른다.

```toml
[project]
name = "my-app"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
some-lib = "1.0.0"

[dev-dependencies]
mock-server = "0.2.0"
```

### 15.2 orv.lock — 의존성 잠금

`orv.lock`은 의존성의 정확한 버전을 기록하는 잠금 파일이다. 자동 생성되며, 버전 관리에 포함한다.

### 15.3 CLI 명령어

| 명령어 | 설명 |
|--------|------|
| `orv build` | 프로젝트 빌드 (서버 바이너리 + 클라이언트 번들) |
| `orv dev` | 개발 서버 실행 (핫 리로드, HMR) |
| `orv test` | 테스트 실행 |
| `orv init` | 새 프로젝트 초기화 |
| `orv add <pkg>` | 의존성 추가 |
| `orv remove <pkg>` | 의존성 제거 |
| `orv db migrate` | DB 스키마 마이그레이션 적용 |
| `orv db backup` | DB 포인트-인-타임 백업 |
| `orv db restore` | DB 복구 |
| `orv db tune` | 런타임 프로파일 기반 쿼리 재최적화 |
| `orv workspace new <name>` | 워크스페이스에 크레이트 추가 |
| `orv graph` | 프로젝트 도메인 그래프 시각화 |

### 15.4 워크스페이스 (멀티 프로젝트)

단일 `orv.toml`이 하나의 바이너리/사이트만 생성하는 것으로 제한되면 조직 규모의 마이크로서비스/모노레포 운영이 불가능하다. 워크스페이스는 여러 orv 크레이트를 하나의 해결(resolve) 단위로 묶는다.

```toml
# 루트 orv.toml
[workspace]
members = ["apps/web", "apps/api", "services/payments", "shared/models"]
resolver = "2"

[workspace.dependencies]
shared-models = { path = "shared/models" }
```

**크로스 프로젝트 타입 공유:** `shared/models`에 선언한 `pub struct User`, `pub enum Role` 등이 `apps/web`과 `services/payments`에서 동일 타입으로 해석된다. RPC 호출 시 컴파일타임 타입 검증이 서비스 경계를 넘어 보장된다.

```orv
// shared/models/src/user.orv
pub struct User { id: int, name: string, email: Email }

// services/payments/src/main.orv
import shared_models.User

let api = @route /api {
  @route POST /charge {
    let body: { user: User, amount: int } = @body
    // User는 web/payments 둘 다에서 동일한 컴파일타임 타입
  }
}
```

**배포 단위:** 각 크레이트는 독립된 서버 바이너리 또는 클라이언트 번들로 출력된다. 워크스페이스 루트의 `orv build` 는 변경된 크레이트만 재빌드한다 (증분, §13.9).

**크로스 크레이트 증분 빌드:** `shared/models` 변경은 해당 타입을 직접 참조한 크레이트만 재컴파일한다. 의존 그래프의 추가 하위 노드는 시그니처 미변경 시 캐시를 유지한다.

**원격 크레이트:**

```toml
[dependencies]
ui-kit = { git = "https://github.com/org/ui-kit", tag = "v2.1.0" }
orv-stripe = "0.3.0"   # 공개 레지스트리
```

공개 레지스트리는 `registry.orv.dev`로 향후 제공된다. 레지스트리 업로드는 버전 이력과 함께 불변 아티팩트로 저장된다.

---

## 16. 통합 에디터

orv 플랫폼은 자체 에디터를 포함한다. 외부 VSCode/Neovim도 LSP/DAP를 통해 지원되지만, **컴파일러 프로젝트 그래프**는 LSP로 직렬화하는 순간 표현력을 잃는다. 자체 에디터는 컴파일러와 같은 메모리 구조를 공유하며, 다음 기능들은 **외부 에디터에서 재현할 수 없는** 것으로 정의된다.

### 16.1 설계 축

1. **라이브 뷰** — 에디터는 파일을 보여주는 것이 아니라 **프로젝트 상태의 투영(projection)**을 보여준다
2. **구조 우선** — 텍스트는 표현 형식일 뿐, 내부 표현은 AST/HIR. 구조 단위 편집이 일급
3. **도메인 가시화** — `@route`, `@db`, `@sync` 같은 도메인 키워드는 색·아이콘·인레이로 경계를 명시
4. **값의 흐름** — `sig` 변수, `await` 결과, `@respond` 페이로드의 현재 값이 코드 옆에 렌더된다
5. **창의성 우선** — 타이핑이 아니라 "보고 고치기"가 기본 편집 모드

### 16.2 토큰 인스펙터

커서가 위치한 토큰의 모든 컴파일타임 정보를 **0ms 지연**으로 표시한다.

| 토큰 | 표시 항목 |
|------|----------|
| 식별자 | 타입, 정의 위치, 참조 개수, RC 전략(단일 소유/공유/Atomic), 캡처 스코프 |
| 함수 호출 | 시그니처, may-throw 전파 경로, 호출 그래프(호출자/피호출자), 배칭 여부 |
| `@db.*` | 쿼리 플랜, 선택된 인덱스, 예상 행 수, 실제 실행 통계 (운영 환경 연결 시) |
| `@route` | 메서드, 프로토콜(QUIC/H2), 미들웨어 체인, RPC facade 포함 여부 |
| `sig` 변수 | 현재 런타임 값, 갱신 도메인(DOM patch/@out replace), 의존 노드 수 |
| 도메인 호출(`@X`) | 정의 위치, property/token 스키마, 중첩 경로 |

인스펙터는 플로팅 박스가 아니라 **우측 상설 패널**로, 커서 이동에 따라 내용만 교체된다. 토큰 타입별 레이아웃은 컴파일러가 타입으로부터 자동 생성한다.

### 16.3 스코프 뎁스 뷰

```
[샘플]
@server {
  @listen 8080             ──── depth 1
  @route GET /api {
    @Auth                  ──── depth 2
    let user = ...         ──── depth 2
    @respond 200 user      ──── depth 2
  }
}
```

- **현재 뎁스 강조** — 커서가 속한 블록은 불투명 100%, 상위는 60%, 하위는 40%로 점진 감쇠
- **뎁스 기반 접기** — `Alt+{숫자}`로 해당 뎁스 이하 모두 접기/펼치기
- **미니맵 세로 게이지** — 파일 우측에 뎁스 기반 컬러 막대. 깊은 곳은 진한 색
- **블록 배경 컬러 코딩** — `@server`=네이비, `@html`=민트, `@db`=앰버 등 도메인별 고정 색. 깊이에 따라 명도 조절

### 16.4 스코프 영역 하이라이팅

커서가 위치한 블록의 **물리적 경계**를 시각화한다.

- **수직 가이드 라인** — 현재 블록 시작~끝을 왼쪽에 굵은 색선으로 연결
- **상위 체인 표시** — 상태표시줄에 `@server > @route GET /api > try { }` 형태로 상위 블록 경로
- **구조 기반 점프** — `Ctrl+Up` = 상위 블록, `Ctrl+Down` = 다음 형제 블록, `Ctrl+Left/Right` = 전/후 형제
- **의미론적 인바운드 하이라이트** — 변수 선언에 커서를 놓으면 **모든 참조**가 동일 색으로 번쩍, 해제 지점은 빨간 점선으로 표시

### 16.5 스플릿 뷰 — 파일 / 라우트 / DB

전통적 파일 트리는 도메인 지향 프로젝트에서 충분하지 않다. orv 에디터는 왼쪽 사이드에 **4개 탭**을 제공한다.

| 탭 | 내용 | 원천 |
|----|------|------|
| **Files** | 일반 파일 트리 | 디렉토리 |
| **Routes** | `@route` / `@ws` / `@wt` / `@webrtc` 엔드포인트 트리 | 프로젝트 그래프 |
| **Schema** | `struct` + `@db.schema` + 인덱스 + 마이그레이션 | 프로젝트 그래프 |
| **Domains** | `define` 커스텀 도메인 + 중첩 계층 | 프로젝트 그래프 |

모든 탭은 **같은 심볼**에 대해 더블클릭 시 에디터에서 정의 위치로 점프한다. Routes 탭은 실제 응답 시간/호출 수(관측 데이터 연결 시)를 옆에 보여준다.

### 16.6 인라인 값 흐름 (Light Table 계승)

`sig` 변수와 `await` 결과는 에디터에 **가상 텍스트**로 현재 값이 표시된다.

```orv
let sig count: int = 0          ▸ 7
let user = await userApi.fetch("/me")   ▸ { id: 42, name: "Alice" }
@respond 200 { ok: true }       ▸ 200  12ms  1.2 KB
```

- 개발 서버 연결 시 실시간 갱신
- `@job`/`@cron` 마지막 실행 결과/상태도 표시
- `Alt+Click`으로 해당 라인의 과거 값 히스토리(최근 50회)

### 16.7 번들 영향 뱃지

모든 함수/도메인/import 선언 옆에 컴파일러가 결정한 **번들 포함 여부**를 표시한다.

| 뱃지 | 의미 |
|------|------|
| ● server | 서버 바이너리에 포함 |
| ● client | 클라이언트 WASM에 포함 |
| ● both | 양쪽 공유 (주로 struct) |
| ○ tree-shaken | DCE로 제거됨 — 사용 안 됨 |
| ⚡ lazy | 라우트 기반 lazy chunk에만 포함 |
| △ ffi | `@unsafe` 영역 — 안전성 보증 밖 |

뱃지 클릭 → 해당 심볼이 reachable해지는 경로(호출자 체인) 표시.

### 16.8 구조화 입력

초보자가 구문 오류에 걸리지 않도록, 에디터는 **키워드 입력 시 문법적으로 올바른 스켈레톤**을 삽입한다.

```
사용자가 @route 타이핑 →
  @route GET / {
    @respond 200 {  }
  }
  위치: 200 옆 body에 커서 자동 배치
```

- `Tab`으로 다음 holes 사이 이동
- 잘못된 자식 도메인(예: `@route` 안의 `@design`)은 **자동 완성 목록에 표시되지 않음**
- 구문 오류 가능성은 크게 낮아지며, 타입 오류는 학습 과정의 일부로 남겨둔다
- 구조화 입력은 초안을 빠르게 만드는 기능이며, 의미/타입 검증은 여전히 컴파일러가 수행한다

### 16.9 빠른 탐색

`Ctrl+P` 범용 팔레트는 단일 입력창에 여러 소스를 쿼리한다.

| 접두사 | 범위 | 예시 |
|--------|------|------|
| (없음) | 파일 + 심볼 | `usrPage` → UserPage.orv |
| `@` | 라우트 | `@GET /api/us` → `@route GET /api/users` |
| `#` | 스키마/struct | `#Ord` → struct Order |
| `!` | 도메인 정의 | `!Lay` → define Layout |
| `>` | 명령 | `> orv db plan` |
| `?` | 문서 | `? @respond` → SPEC.md §11.4로 점프 |

퍼지 매칭 결과는 컴파일러 심볼 테이블을 직접 참조하므로 **별도 인덱싱 없이** 즉시 뜬다.

### 16.10 협업 편집

내장 CRDT(§11.20)를 그대로 사용한다. 별도 서버/프로토콜 없이 `orv dev --share` 한 줄로 세션이 공유되고, 커서/선택/편집이 실시간 동기화된다. 에디터 자체가 Orv로 쓰였기 때문에 언어의 모든 기능이 에디터 내에서도 같은 원리로 동작한다 (self-host).

### 16.11 디자인 편집 모드 (layouts.dev 계승)

비개발자 타겟을 위한 "창의성 우선" 모드이다. `@html` 블록에 커서가 있을 때 `Alt+D`로 진입한다.

- **WYSIWYG 편집** — 실제 렌더된 페이지에서 마우스로 요소 선택/수정
- **스타일 툴바** — `@design` 토큰을 드롭다운으로 적용, 값 변경 시 코드가 함께 갱신
- **다기기 실시간 미리보기** — 모바일/태블릿/데스크톱 동시 렌더
- **컴포넌트 갤러리** — `pub define`된 도메인을 시각 카드로 브라우징 후 드래그 삽입
- **코드 + 프리뷰 양방향 동기** — 코드 수정이 프리뷰에, 프리뷰 수정이 코드에 즉시 반영

이 모드는 동일한 `.orv` 파일을 편집하며, 별도 저장 포맷이 없다.

### 16.12 런타임 인스펙션

에디터는 개발 서버와 **바이너리 RPC**로 연결되어 런타임 상태를 즉시 노출한다.

- **프로젝트 그래프 뷰어** — 라우트-도메인-스키마의 관계를 인터랙티브 그래프로 표시
- **쿼리 플랜 시각화** — `@db` 호출을 클릭하면 실행 플랜 트리와 인덱스 히트 표시
- **CRDT 타임라인** — `@sync` 문서의 오퍼레이션 로그와 머지 지점을 시간축에 표시
- **번들 시각화** — 각 청크의 크기, 포함 모듈, 의존 그래프

### 16.13 성능 목표

| 항목 | 목표 |
|------|------|
| 키입력 → 화면 반영 | ≤ 8 ms (120Hz 모니터 한 프레임) |
| 토큰 인스펙터 갱신 | ≤ 16 ms |
| 심볼 점프 (팔레트) | ≤ 30 ms (수십만 심볼 프로젝트) |
| 디자인 모드 리렌더 | ≤ 33 ms (30fps) |
| 증분 빌드 반영 | ≤ 1 s (1M LOC 기준 단일 파일 변경) |
| 협업 동기화 지연 | ≤ 100 ms (로컬 네트워크) |

에디터 자체는 Orv 런타임 위에서 wgpu 기반 GPU 렌더링을 사용한다 (`@gpu` §10.11 활용).

### 16.14 외부 에디터 지원

자체 에디터를 권장하지만, VSCode/Neovim/JetBrains도 1급으로 지원한다.

- **LSP 서버**: `orv lsp` — 자동완성, 진단, 정의 이동, 리팩터링
- **DAP 서버**: 디버거 프로토콜 (중단점, 스텝, 변수 관찰)
- **Tree-sitter 문법**: 구문 강조와 구조 선택
- **한계**: 프로젝트 그래프의 동적 탐색, sig 인라인 값, 번들 영향 뱃지, 디자인 모드는 자체 에디터 전용

---

> 통합 예제(풀스택 구성)는 `fixtures/plan/07-fullstack-showcase.orv`를 참조하라.
> 슈퍼앱 시뮬레이션은 `fixtures/plan/08-superapp-simulation.orv`를 참조하라.
