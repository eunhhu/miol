export const ORV_CLIENT_BOOTSTRAP = Object.freeze(__ORV_BOOTSTRAP__);

const manifestUrl = new URL(ORV_CLIENT_BOOTSTRAP.manifestUrl, import.meta.url);
const reactivePlanUrl = new URL(ORV_CLIENT_BOOTSTRAP.reactivePlanUrl, import.meta.url);
const wasmUrl = new URL(ORV_CLIENT_BOOTSTRAP.wasmUrl, import.meta.url);
const sourceBundleUrl = new URL(ORV_CLIENT_BOOTSTRAP.sourceBundleUrl, import.meta.url);
const root = document.querySelector('[data-orv-client="wasm"]');

const FNV_OFFSET = 0xcbf29ce484222325n;
const FNV_PRIME = 0x100000001b3n;

function fnv1a64(bytes) {
  let hash = FNV_OFFSET;
  for (const byte of bytes) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * FNV_PRIME);
  }
  return hash.toString(16).padStart(16, "0");
}

function stableJsonHash(value) {
  return fnv1a64(new TextEncoder().encode(JSON.stringify(value)));
}

async function loadClientManifest() {
  const response = await fetch(manifestUrl);
  if (!response.ok) {
    throw new Error(`orv client manifest fetch failed: ${response.status}`);
  }
  const manifest = await response.json();
  if (manifest.schema_version !== 1 || manifest.kind !== "orv.client.bundle") {
    throw new Error("orv client manifest schema mismatch");
  }
  if (manifest.source_bundle !== ORV_CLIENT_BOOTSTRAP.manifestSourceBundle) {
    throw new Error("orv client manifest source bundle mismatch");
  }
  if (manifest.source_bundle_hash !== ORV_CLIENT_BOOTSTRAP.sourceBundleHash) {
    throw new Error(`orv client manifest hash mismatch: expected ${ORV_CLIENT_BOOTSTRAP.sourceBundleHash}, got ${manifest.source_bundle_hash}`);
  }
  if (manifest.reactive_plan !== ORV_CLIENT_BOOTSTRAP.manifestReactivePlan) {
    throw new Error("orv client manifest reactive plan mismatch");
  }
  if (manifest.wasm !== ORV_CLIENT_BOOTSTRAP.manifestWasm) {
    throw new Error("orv client manifest wasm mismatch");
  }
  const exports = manifest.exports || {};
  if (
    exports.start !== ORV_CLIENT_BOOTSTRAP.exports.start ||
    exports.render_ptr !== ORV_CLIENT_BOOTSTRAP.exports.renderPtr ||
    exports.render_len !== ORV_CLIENT_BOOTSTRAP.exports.renderLen ||
    exports.memory !== ORV_CLIENT_BOOTSTRAP.exports.memory
  ) {
    throw new Error("orv client manifest export mismatch");
  }
  return manifest;
}

async function loadReactivePlan(manifest) {
  const response = await fetch(reactivePlanUrl);
  if (!response.ok) {
    throw new Error(`orv client reactive plan fetch failed: ${response.status}`);
  }
  const plan = await response.json();
  if (plan.schema_version !== 1 || plan.kind !== "orv.client.reactive_plan") {
    throw new Error("orv client reactive plan schema mismatch");
  }
  if (plan.source_bundle !== manifest.source_bundle) {
    throw new Error("orv client reactive plan source bundle mismatch");
  }
  if (plan.source_bundle_hash !== manifest.source_bundle_hash) {
    throw new Error(`orv client reactive plan hash mismatch: expected ${manifest.source_bundle_hash}, got ${plan.source_bundle_hash}`);
  }
  if (plan.entry !== manifest.entry) {
    throw new Error("orv client reactive plan entry mismatch");
  }
  if (!Array.isArray(plan.signals)) {
    throw new Error("orv client reactive plan signals mismatch");
  }
  if (!plan.signals.every((signal) =>
    typeof signal.name === "string" &&
    typeof signal.origin_id === "string" &&
    typeof signal.state_key === "string" &&
    signal.initial_value &&
    typeof signal.initial_value.kind === "string"
  )) {
    throw new Error("orv client reactive plan signal metadata mismatch");
  }
  validateReactiveBindings(plan, manifest);
  return plan;
}

function validateReactiveBindings(plan, manifest) {
  if (!Array.isArray(plan.bindings)) {
    throw new Error("orv client reactive plan bindings mismatch");
  }
  const hasInitialRenderBinding = plan.bindings.some((binding) =>
    binding.kind === "initial_render" &&
    binding.target === manifest.page &&
    binding.source === manifest.wasm
  );
  if (!hasInitialRenderBinding) {
    throw new Error("orv client reactive plan initial_render binding mismatch");
  }
  const hasSignalStateBindings = plan.signals.every((signal) =>
    plan.bindings.some((binding) =>
      binding.kind === "signal_state" &&
      binding.target === manifest.loader &&
      binding.source === signal.origin_id &&
      binding.state_key === signal.state_key
    )
  );
  if (!hasSignalStateBindings) {
    throw new Error("orv client reactive plan signal_state binding mismatch");
  }
  const signalTextBindingsAreValid = plan.bindings
    .filter((binding) => binding.kind === "signal_text")
    .every((binding) =>
      binding.target === manifest.page &&
      typeof binding.selector === "string" &&
      binding.selector.length > 0 &&
      plan.signals.some((signal) =>
        binding.source === signal.origin_id &&
        binding.state_key === signal.state_key
      ) &&
      signalTextBindingStateKeysAreValid(binding, plan.signals) &&
      signalTextSourcesAreValid(binding, plan.signals) &&
      signalTextTemplateIsValid(binding, plan.signals) &&
      signalTextConditionIsValid(binding, plan.signals)
    );
  if (!signalTextBindingsAreValid) {
    throw new Error("orv client reactive plan signal_text binding mismatch");
  }
  const signalAttrBindingsAreValid = plan.bindings
    .filter((binding) => binding.kind === "signal_attr")
    .every((binding) =>
      binding.target === manifest.page &&
      typeof binding.selector === "string" &&
      binding.selector.length > 0 &&
      typeof binding.attr === "string" &&
      binding.attr.length > 0 &&
      plan.signals.some((signal) =>
        binding.source === signal.origin_id &&
        binding.state_key === signal.state_key
      ) &&
      signalAttrBindingStateKeysAreValid(binding, plan.signals) &&
      signalAttrSourcesAreValid(binding, plan.signals) &&
      signalAttrTemplateIsValid(binding, plan.signals) &&
      signalAttrConditionIsValid(binding, plan.signals)
    );
  if (!signalAttrBindingsAreValid) {
    throw new Error("orv client reactive plan signal_attr binding mismatch");
  }
  const signalEventBindingsAreValid = plan.bindings
    .filter((binding) => binding.kind === "signal_event")
    .every((binding) =>
      binding.target === manifest.page &&
      typeof binding.selector === "string" &&
      binding.selector.length > 0 &&
      typeof binding.event === "string" &&
      binding.event.length > 0 &&
      binding.action &&
      (
        [
          "assign_toggle",
          "assign_event_target_checked",
          "assign_event_target_value",
          "assign_event_target_value_float",
          "assign_event_target_value_int",
        ].includes(binding.action.kind) ||
        (
          ["assign", "assign_add", "assign_sub"].includes(binding.action.kind) &&
          binding.action.value &&
          typeof binding.action.value.kind === "string"
        )
      ) &&
      plan.signals.some((signal) =>
        binding.source === signal.origin_id &&
        binding.state_key === signal.state_key
      )
    );
  if (!signalEventBindingsAreValid) {
    throw new Error("orv client reactive plan signal_event binding mismatch");
  }
}

function signalTextTemplateIsValid(binding, signals) {
  if (binding.text_template === undefined) {
    return true;
  }
  const stateKeys = signalTextBindingStateKeys(binding);
  return Array.isArray(binding.text_template) &&
    binding.text_template.length > 0 &&
    binding.text_template.every((segment) => {
      if (segment.kind === "text") {
        return typeof segment.value === "string";
      }
      if (segment.kind === "signal") {
        return typeof segment.state_key === "string" &&
          stateKeys.includes(segment.state_key) &&
          signals.some((signal) => signal.state_key === segment.state_key);
      }
      return false;
    });
}

function signalTextConditionIsValid(binding, signals) {
  if (binding.text_condition === undefined) {
    return true;
  }
  return signalConditionIsValidForBinding(binding, signals, binding.text_condition);
}

function signalTextBindingStateKeys(binding) {
  if (Array.isArray(binding.state_keys)) {
    return binding.state_keys;
  }
  return typeof binding.state_key === "string" ? [binding.state_key] : [];
}

function signalTextBindingStateKeysAreValid(binding, signals) {
  const stateKeys = signalTextBindingStateKeys(binding);
  return stateKeys.length > 0 &&
    stateKeys.every((stateKey) => typeof stateKey === "string") &&
    stateKeys.includes(binding.state_key) &&
    stateKeys.every((stateKey) => signals.some((signal) => signal.state_key === stateKey));
}

function signalTextSourcesAreValid(binding, signals) {
  if (binding.sources === undefined) {
    return true;
  }
  const stateKeys = signalTextBindingStateKeys(binding);
  return Array.isArray(binding.sources) &&
    binding.sources.length > 0 &&
    binding.sources.every((source) =>
      source &&
      typeof source.source === "string" &&
      typeof source.state_key === "string" &&
      stateKeys.includes(source.state_key) &&
      signals.some((signal) =>
        signal.origin_id === source.source &&
        signal.state_key === source.state_key
      )
    );
}

function signalAttrTemplateIsValid(binding, signals) {
  if (binding.attr_template === undefined) {
    return true;
  }
  const stateKeys = signalAttrBindingStateKeys(binding);
  return Array.isArray(binding.attr_template) &&
    binding.attr_template.length > 0 &&
    binding.attr_template.every((segment) => {
      if (segment.kind === "text") {
        return typeof segment.value === "string";
      }
      if (segment.kind === "signal") {
        return typeof segment.state_key === "string" &&
          stateKeys.includes(segment.state_key) &&
          signals.some((signal) => signal.state_key === segment.state_key);
      }
      return false;
    });
}

function signalAttrBindingStateKeys(binding) {
  if (Array.isArray(binding.state_keys)) {
    return binding.state_keys;
  }
  return typeof binding.state_key === "string" ? [binding.state_key] : [];
}

function signalAttrBindingStateKeysAreValid(binding, signals) {
  const stateKeys = signalAttrBindingStateKeys(binding);
  return stateKeys.length > 0 &&
    stateKeys.every((stateKey) => typeof stateKey === "string") &&
    stateKeys.includes(binding.state_key) &&
    stateKeys.every((stateKey) => signals.some((signal) => signal.state_key === stateKey));
}

function signalAttrSourcesAreValid(binding, signals) {
  if (binding.sources === undefined) {
    return true;
  }
  const stateKeys = signalAttrBindingStateKeys(binding);
  return Array.isArray(binding.sources) &&
    binding.sources.length > 0 &&
    binding.sources.every((source) =>
      source &&
      typeof source.source === "string" &&
      typeof source.state_key === "string" &&
      stateKeys.includes(source.state_key) &&
      signals.some((signal) =>
        signal.origin_id === source.source &&
        signal.state_key === source.state_key
      )
    );
}

function signalAttrConditionIsValid(binding, signals) {
  if (binding.attr_condition === undefined) {
    return true;
  }
  return signalConditionIsValidForBinding(binding, signals, binding.attr_condition);
}

function signalConditionIsValidForBinding(binding, signals, condition) {
  return condition &&
    typeof condition.state_key === "string" &&
    condition.state_key === binding.state_key &&
    typeof condition.truthy === "string" &&
    typeof condition.falsy === "string" &&
    signalAttrConditionComparisonIsValid(condition) &&
    signals.some((signal) => signal.state_key === condition.state_key);
}

function signalAttrConditionComparisonIsValid(condition) {
  const hasOp = condition.op !== undefined;
  const hasRhs = condition.rhs !== undefined;
  if (!hasOp && !hasRhs) {
    return true;
  }
  return hasOp &&
    hasRhs &&
    ["eq", "ne", "lt", "gt", "le", "ge"].includes(condition.op) &&
    signalConditionOperandIsValid(condition.rhs);
}

function signalConditionOperandIsValid(operand) {
  if (!operand || typeof operand.kind !== "string") {
    return false;
  }
  if (["int", "float", "string"].includes(operand.kind)) {
    return typeof operand.value === "string";
  }
  if (operand.kind === "bool") {
    return typeof operand.value === "boolean";
  }
  return false;
}

function decodeSignalInitialValue(metadata) {
  switch (metadata.kind) {
    case "int":
    case "float":
      return Number(metadata.value);
    case "string":
      return String(metadata.value ?? "");
    case "bool":
      return Boolean(metadata.value);
    case "void":
      return null;
    default:
      return metadata.value ?? null;
  }
}

function createReactiveState(plan) {
  return Object.fromEntries(plan.signals.map((signal) => [
    signal.state_key,
    {
      origin_id: signal.origin_id,
      value: decodeSignalInitialValue(signal.initial_value),
      initial_value: signal.initial_value,
    },
  ]));
}

function displaySignalValue(value) {
  return value == null ? "" : String(value);
}

function renderSignalTextBinding(binding, reactiveState) {
  if (binding.text_condition) {
    return renderSignalTextCondition(binding.text_condition, reactiveState);
  }
  if (!Array.isArray(binding.text_template)) {
    return displaySignalValue(reactiveState[binding.state_key]?.value);
  }
  return binding.text_template.map((segment) => {
    if (segment.kind === "text") {
      return segment.value;
    }
    if (segment.kind === "signal") {
      return displaySignalValue(reactiveState[segment.state_key]?.value);
    }
    return "";
  }).join("");
}

function renderSignalTextCondition(condition, reactiveState) {
  return renderSignalAttrCondition(condition, reactiveState);
}

function bindReactiveDom(plan, root, reactiveState) {
  const bindings = new Map();
  const boundElements = new Set();
  if (!root) {
    return { count: 0, update() {} };
  }
  const textBindings = plan.bindings.filter((binding) => binding.kind === "signal_text");
  for (const binding of textBindings) {
    const stateKeys = signalTextBindingStateKeys(binding);
    if (!stateKeys.every((stateKey) => reactiveState[stateKey])) {
      continue;
    }
    const expectedText = renderSignalTextBinding(binding, reactiveState);
    const element = Array.from(root.querySelectorAll(binding.selector))
      .find((candidate) => candidate.textContent === expectedText);
    if (!element) {
      continue;
    }
    element.dataset.orvSignal = binding.state_key;
    element.dataset.orvSignalKeys = stateKeys.join(",");
    boundElements.add(element);
    for (const stateKey of stateKeys) {
      const current = bindings.get(stateKey) || [];
      current.push({ element, binding });
      bindings.set(stateKey, current);
    }
  }
  return {
    count: boundElements.size,
    update(stateKey) {
      const items = bindings.get(stateKey) || [];
      for (const item of items) {
        item.element.textContent = renderSignalTextBinding(item.binding, reactiveState);
      }
    },
  };
}

function elementSignalAttrValue(element, attr) {
  if (attr in element && typeof element[attr] !== "function") {
    return element[attr];
  }
  return element.getAttribute(attr);
}

function setElementSignalAttr(element, attr, value) {
  if (attr === "checked") {
    element.checked = Boolean(value);
    if (element.checked) {
      element.setAttribute(attr, "");
    } else {
      element.removeAttribute(attr);
    }
    return;
  }
  const text = displaySignalValue(value);
  if (attr in element && typeof element[attr] !== "function") {
    element[attr] = value == null ? "" : value;
  }
  element.setAttribute(attr, text);
}

function renderSignalAttrBinding(binding, reactiveState) {
  if (!Array.isArray(binding.attr_template)) {
    if (binding.attr_condition) {
      return renderSignalAttrCondition(binding.attr_condition, reactiveState);
    }
    return reactiveState[binding.state_key]?.value;
  }
  return binding.attr_template.map((segment) => {
    if (segment.kind === "text") {
      return segment.value;
    }
    if (segment.kind === "signal") {
      return displaySignalValue(reactiveState[segment.state_key]?.value);
    }
    return "";
  }).join("");
}

function renderSignalAttrCondition(condition, reactiveState) {
  const matched = condition.op ?
    compareSignalAttrCondition(condition, reactiveState) :
    Boolean(reactiveState[condition.state_key]?.value);
  return matched ? condition.truthy : condition.falsy;
}

function compareSignalAttrCondition(condition, reactiveState) {
  const value = reactiveState[condition.state_key]?.value;
  const rhs = decodeSignalConditionOperand(condition.rhs);
  switch (condition.op) {
    case "eq":
      return value === rhs;
    case "ne":
      return value !== rhs;
    case "lt":
      return value < rhs;
    case "gt":
      return value > rhs;
    case "le":
      return value <= rhs;
    case "ge":
      return value >= rhs;
    default:
      return false;
  }
}

function decodeSignalConditionOperand(operand) {
  switch (operand.kind) {
    case "int":
    case "float":
      return Number(operand.value);
    case "string":
      return String(operand.value ?? "");
    case "bool":
      return Boolean(operand.value);
    default:
      return null;
  }
}

function bindReactiveAttrs(plan, root, reactiveState) {
  const bindings = new Map();
  const boundElements = new Set();
  if (!root) {
    return { count: 0, update() {} };
  }
  const attrBindings = plan.bindings.filter((binding) => binding.kind === "signal_attr");
  for (const binding of attrBindings) {
    const stateKeys = signalAttrBindingStateKeys(binding);
    if (!stateKeys.every((stateKey) => reactiveState[stateKey])) {
      continue;
    }
    const expected = displaySignalValue(renderSignalAttrBinding(binding, reactiveState));
    const element = Array.from(root.querySelectorAll(binding.selector))
      .find((candidate) => displaySignalValue(elementSignalAttrValue(candidate, binding.attr)) === expected);
    if (!element) {
      continue;
    }
    element.dataset.orvSignalAttr = binding.state_key;
    element.dataset.orvSignalAttrKeys = stateKeys.join(",");
    boundElements.add(element);
    for (const stateKey of stateKeys) {
      const current = bindings.get(stateKey) || [];
      current.push({ element, binding });
      bindings.set(stateKey, current);
    }
  }
  return {
    count: boundElements.size,
    update(stateKey) {
      const attrs = bindings.get(stateKey) || [];
      for (const item of attrs) {
        setElementSignalAttr(
          item.element,
          item.binding.attr,
          renderSignalAttrBinding(item.binding, reactiveState),
        );
      }
    },
  };
}

function signalEventAttributeName(eventName) {
  return `on${eventName.charAt(0).toUpperCase()}${eventName.slice(1)}`;
}

function applySignalAction(action, currentValue, event) {
  switch (action.kind) {
    case "assign":
      return decodeSignalInitialValue(action.value);
    case "assign_add":
      return currentValue + decodeSignalInitialValue(action.value);
    case "assign_sub":
      return currentValue - decodeSignalInitialValue(action.value);
    case "assign_toggle":
      return !Boolean(currentValue);
    case "assign_event_target_checked":
      return Boolean(event?.target?.checked);
    case "assign_event_target_value":
      return event?.target?.value ?? "";
    case "assign_event_target_value_float":
      return Number.parseFloat(event?.target?.value ?? "0");
    case "assign_event_target_value_int":
      return Number.parseInt(event?.target?.value ?? "0", 10);
    default:
      throw new Error(`orv client signal event action is unsupported: ${action.kind}`);
  }
}

function bindReactiveEvents(plan, root, reactiveState, setSignal) {
  if (!root) {
    return { count: 0 };
  }
  let count = 0;
  const eventBindings = plan.bindings.filter((binding) => binding.kind === "signal_event");
  const cursors = new Map();
  for (const binding of eventBindings) {
    const state = reactiveState[binding.state_key];
    if (!state) {
      continue;
    }
    const attr = signalEventAttributeName(binding.event);
    const key = `${binding.selector}\u0000${binding.event}`;
    const cursor = cursors.get(key) || 0;
    const candidates = Array.from(root.querySelectorAll(binding.selector))
      .filter((element) => element.getAttribute(attr) === "handler" || element.getAttribute(attr.toLowerCase()) === "handler");
    const element = candidates[cursor];
    cursors.set(key, cursor + 1);
    if (!element) {
      continue;
    }
    element.dataset.orvSignalEvent = binding.state_key;
    element.addEventListener(binding.event, (event) => {
      setSignal(binding.state_key, applySignalAction(binding.action, state.value, event));
    });
    count += 1;
  }
  return { count };
}

async function loadSourceBundle(manifest) {
  const response = await fetch(sourceBundleUrl);
  if (!response.ok) {
    throw new Error(`orv source bundle fetch failed: ${response.status}`);
  }
  const sourceBundle = await response.json();
  const actualHash = stableJsonHash(sourceBundle);
  if (actualHash !== manifest.source_bundle_hash) {
    throw new Error(`orv source bundle hash mismatch: expected ${manifest.source_bundle_hash}, got ${actualHash}`);
  }
  return sourceBundle;
}

function readInitialRender(instance) {
  const { memory, orv_render_ptr, orv_render_len } = instance.exports;
  if (!(memory instanceof WebAssembly.Memory) || typeof orv_render_ptr !== "function" || typeof orv_render_len !== "function") {
    return "";
  }
  const ptr = Number(orv_render_ptr());
  const len = Number(orv_render_len());
  if (!Number.isSafeInteger(ptr) || !Number.isSafeInteger(len) || ptr < 0 || len < 0) {
    throw new Error("orv client render exports returned invalid bounds");
  }
  return new TextDecoder().decode(new Uint8Array(memory.buffer, ptr, len));
}

function validateInitialRender(manifest, html) {
  const expected = manifest.initial_render || {};
  if (expected.content_type !== "text/html" || expected.encoding !== "utf-8") {
    throw new Error("orv client initial render metadata mismatch");
  }
  const bytes = new TextEncoder().encode(html);
  const actualHash = fnv1a64(bytes);
  if (actualHash !== expected.html_hash) {
    throw new Error(`orv client initial render hash mismatch: expected ${expected.html_hash}, got ${actualHash}`);
  }
  if (bytes.length !== expected.byte_length) {
    throw new Error(`orv client initial render byte length mismatch: expected ${expected.byte_length}, got ${bytes.length}`);
  }
}

function validateWasmBundle(manifest, bytes) {
  const actualHash = fnv1a64(new Uint8Array(bytes));
  if (actualHash !== manifest.wasm_hash) {
    throw new Error(`orv client wasm hash mismatch: expected ${manifest.wasm_hash}, got ${actualHash}`);
  }
}

async function main() {
  const manifest = await loadClientManifest();
  const reactivePlan = await loadReactivePlan(manifest);
  const reactiveState = createReactiveState(reactivePlan);
  const sourceBundle = await loadSourceBundle(manifest);
  const response = await fetch(wasmUrl);
  const bytes = await response.arrayBuffer();
  validateWasmBundle(manifest, bytes);
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const initialRender = readInitialRender(instance);
  validateInitialRender(manifest, initialRender);
  if (root && initialRender) {
    root.innerHTML = initialRender;
  }
  const reactiveDom = bindReactiveDom(reactivePlan, root, reactiveState);
  let reactiveAttrs = { count: 0, update() {} };
  function setSignal(stateKey, value) {
    const state = reactiveState[stateKey];
    if (!state) {
      throw new Error(`orv client signal state not found: ${stateKey}`);
    }
    state.value = value;
    reactiveDom.update(stateKey, value);
    reactiveAttrs.update(stateKey, value);
    if (root) {
      root.dataset.orvReactiveStateHash = stableJsonHash(reactiveState);
    }
  }
  reactiveAttrs = bindReactiveAttrs(reactivePlan, root, reactiveState);
  const reactiveEvents = bindReactiveEvents(reactivePlan, root, reactiveState, setSignal);
  if (typeof instance.exports.orv_start === "function") {
    instance.exports.orv_start();
  }
  if (root) {
    root.dataset.orvStatus = "ready";
    root.dataset.orvClientManifest = manifestUrl.href;
    root.dataset.orvSourceBundle = sourceBundleUrl.href;
    root.dataset.orvSourceBundleHash = manifest.source_bundle_hash;
    root.dataset.orvEntry = ORV_CLIENT_BOOTSTRAP.entry;
    root.dataset.orvSourceCount = String(sourceBundle.files?.length ?? 0);
    root.dataset.orvReactiveSignals = String(reactivePlan.signals.length);
    root.dataset.orvReactiveBindings = String(reactivePlan.bindings.filter((binding) => binding.kind === "signal_state").length);
    root.dataset.orvReactiveDomBindings = String(reactiveDom.count);
    root.dataset.orvReactiveAttrBindings = String(reactiveAttrs.count);
    root.dataset.orvReactiveEventBindings = String(reactiveEvents.count);
    root.dataset.orvReactiveStateHash = stableJsonHash(reactiveState);
  }
  globalThis.__ORV_CLIENT_REACTIVE_STATE__ = Object.freeze(reactiveState);
  globalThis.__ORV_SET_SIGNAL__ = setSignal;
}

main().catch((error) => {
  console.error("orv client bootstrap failed", error);
  if (root) {
    root.dataset.orvStatus = "error";
  }
});
