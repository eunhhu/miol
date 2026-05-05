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
      )
    );
  if (!signalTextBindingsAreValid) {
    throw new Error("orv client reactive plan signal_text binding mismatch");
  }
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

function bindReactiveDom(plan, root, reactiveState) {
  const bindings = new Map();
  if (!root) {
    return { count: 0, update() {} };
  }
  const textBindings = plan.bindings.filter((binding) => binding.kind === "signal_text");
  for (const binding of textBindings) {
    const state = reactiveState[binding.state_key];
    if (!state) {
      continue;
    }
    const expectedText = displaySignalValue(state.value);
    const element = Array.from(root.querySelectorAll(binding.selector))
      .find((candidate) => candidate.textContent === expectedText);
    if (!element) {
      continue;
    }
    element.dataset.orvSignal = binding.state_key;
    const current = bindings.get(binding.state_key) || [];
    current.push(element);
    bindings.set(binding.state_key, current);
  }
  return {
    count: [...bindings.values()].reduce((total, items) => total + items.length, 0),
    update(stateKey, value) {
      const elements = bindings.get(stateKey) || [];
      for (const element of elements) {
        element.textContent = displaySignalValue(value);
      }
    },
  };
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
  function setSignal(stateKey, value) {
    const state = reactiveState[stateKey];
    if (!state) {
      throw new Error(`orv client signal state not found: ${stateKey}`);
    }
    state.value = value;
    reactiveDom.update(stateKey, value);
    if (root) {
      root.dataset.orvReactiveStateHash = stableJsonHash(reactiveState);
    }
  }
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
