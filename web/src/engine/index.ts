// Loads the Rust/WebAssembly engine once (streaming compile via wasm-bindgen's
// `init`, which uses WebAssembly.instantiateStreaming when served as application/wasm).
import init, { FlexmaEditor } from './pkg/flexma_engine';
import wasmUrl from './pkg/flexma_engine_bg.wasm?url';

let ready: Promise<unknown> | null = null;

export function loadEngine(): Promise<unknown> {
  if (!ready) {
    ready = init({ module_or_path: wasmUrl }).catch((err: unknown) => {
      ready = null;
      throw err;
    });
  }
  return ready;
}

export { FlexmaEditor };
