// Loads the Rust/WebAssembly engine once (streaming compile via wasm-bindgen's
// `init`, which uses WebAssembly.instantiateStreaming when served as application/wasm),
// and creates editors that draw with WebGPU, or WebGL2 where WebGPU isn't available.
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

export type RendererChoice = 'webgpu' | 'webgl2';

/** Set when WebGPU failed in this browser; the editor then uses WebGL2. */
const FALLBACK_KEY = 'flexma:rendererFallback';

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/**
 * The renderer to try. `?renderer=webgl` or `?renderer=webgpu` in the URL
 * overrides it (and `webgpu` clears a remembered failure).
 */
export function preferredRenderer(): RendererChoice {
  const param = new URLSearchParams(window.location.search).get('renderer');
  if (param === 'webgl' || param === 'webgl2') return 'webgl2';
  if (param === 'webgpu') {
    storage()?.removeItem(FALLBACK_KEY);
    return 'webgpu';
  }
  return storage()?.getItem(FALLBACK_KEY) ? 'webgl2' : 'webgpu';
}

// Just the parts of the WebGPU API used here (TypeScript's DOM types don't include it).
interface GpuDeviceLike {
  lost: Promise<{ reason: string; message: string }>;
  addEventListener(type: 'uncapturederror', fn: (e: { error: { message: string } }) => void): void;
  destroy(): void;
}
interface GpuAdapterLike {
  requestDevice(desc?: object): Promise<GpuDeviceLike>;
}
interface GpuLike {
  requestAdapter(opts?: object): Promise<GpuAdapterLike | null>;
  getPreferredCanvasFormat(): string;
}
interface GpuContextLike {
  configure(desc: object): void;
}

export interface CreateEditorOptions {
  /**
   * WebGPU broke (a validation error or a lost device). The browser now
   * prefers WebGL2; save what you need, then reload. Default: reload.
   */
  onGpuFailure?: (message: string) => void;
}

/**
 * Most paint passes the engine encodes ahead of the GPU (see
 * `FlexmaEditor.withWebGpu`). iPads and iPhones (Apple's engine with a touch
 * screen) get the smaller budget their GPU process memory allows.
 */
function paintCap(): number {
  const appleMobile = navigator.vendor.startsWith('Apple') && navigator.maxTouchPoints > 1;
  return appleMobile ? 1024 : 4096;
}

/** A new editor drawing on `canvas` with the best available renderer. */
export async function createEditor(canvas: HTMLCanvasElement, opts: CreateEditorOptions = {}): Promise<FlexmaEditor> {
  await loadEngine();
  const gpu = (navigator as Navigator & { gpu?: GpuLike }).gpu;
  if (preferredRenderer() === 'webgpu' && gpu) {
    let device: GpuDeviceLike | null = null;
    try {
      // Ask for the faster GPU on machines with two; any GPU if that fails.
      const adapter = (await gpu.requestAdapter({ powerPreference: 'high-performance' })) ?? (await gpu.requestAdapter());
      device = adapter ? await adapter.requestDevice() : null;
    } catch (err) {
      console.warn('WebGPU is unavailable, drawing with WebGL2 instead.', err);
    }
    if (device) {
      const context = canvas.getContext('webgpu') as unknown as GpuContextLike | null;
      if (context) {
        const format = gpu.getPreferredCanvasFormat();
        context.configure({ device, format, alphaMode: 'premultiplied' });
        // Asked for explicitly (?renderer=webgpu): report errors, don't switch.
        const forced = new URLSearchParams(window.location.search).get('renderer') === 'webgpu';
        let failed = false;
        const fail = (message: string) => {
          console.error(`WebGPU: ${message}`);
          if (failed || forced) return;
          failed = true;
          console.error(`WebGPU failed (${message}); switching this browser to WebGL2.`);
          storage()?.setItem(FALLBACK_KEY, message.slice(0, 300));
          (opts.onGpuFailure ?? (() => window.location.reload()))(message);
        };
        device.addEventListener('uncapturederror', (e) => fail(e.error.message));
        // Flexma never destroys its device while the editor is open, so any
        // loss then is a failure. Safari reports the system ending its GPU
        // process (an iPad out of memory) as "destroyed" too.
        void device.lost.then((info) => {
          if (canvas.isConnected) fail(`device lost (${info.reason}): ${info.message}`);
        });
        try {
          const editor = FlexmaEditor.withWebGpu(device, context, format, (message: string) => fail(message), paintCap());
          // Safari throws from WebGPU calls (on a lost device, say) where
          // Chrome reports an error later. A throw inside the engine leaves it
          // unusable, so the first one switches to WebGL2 like any GPU failure.
          const frame = editor.frame.bind(editor);
          editor.frame = (now: number) => {
            try {
              return frame(now);
            } catch (err) {
              fail(String(err));
              throw err;
            }
          };
          return editor;
        } catch (err) {
          // The canvas now has a WebGPU context and can't switch to WebGL2.
          fail(String(err));
          throw err;
        }
      }
      device.destroy();
    }
  }
  return new FlexmaEditor(canvas);
}
