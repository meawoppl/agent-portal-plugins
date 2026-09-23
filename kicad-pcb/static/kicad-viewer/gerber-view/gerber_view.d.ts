/* tslint:disable */
/* eslint-disable */

/**
 * Embeddable Gerber viewer. Create one, `mount` it into an element with a size,
 * then give it sources with `setSources`.
 */
export class GerberViewer {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Add `sources` to the current layers. Accepts the same forms as `setSources`.
     */
    addSources(sources: any): Promise<any>;
    /**
     * Remove all layers and diagnostics. Pending loads from earlier calls are discarded.
     */
    clear(): void;
    /**
     * Unmount from the DOM and drop all layers. Call `free()` afterwards to release
     * the WebAssembly memory held by this object.
     */
    destroy(): void;
    /**
     * Full parsed geometry as a JSON string, in the same shape `parseSources` returns.
     */
    exportProject(): string;
    /**
     * Fit the board in the viewport. The view keeps refitting on resize and new
     * layers until the user pans or zooms.
     */
    fit(): void;
    /**
     * Render into `container`, which should have a non-zero size. The viewer fills
     * it and tracks its size. Mounting again moves the viewer to the new container.
     */
    mount(container: HTMLElement): void;
    /**
     * Options (all optional): `{ background: "#12161c", controls: false, padding: 16 }`.
     */
    constructor(options: any);
    /**
     * Register a callback invoked with the project summary whenever layers, styles,
     * or the viewing side change. Pass `null` to remove it.
     */
    onChange(callback?: Function | null): void;
    /**
     * Summary of the loaded project: layers (name, label, function, side, colour,
     * visibility, counts, bbox), board bbox in millimetres, `warnings` (problems),
     * `skipped` (non-fabrication or empty files), and view side.
     */
    project(): any;
    /**
     * Re-read the container size. Only needed where `ResizeObserver` is unavailable
     * and the container changes size without a window resize.
     */
    resize(): void;
    /**
     * Set a layer's colour (any CSS colour) by source name or label.
     */
    setLayerColor(layer: string, color: string): boolean;
    /**
     * Set a layer's opacity (0 to 1) by source name or label.
     */
    setLayerOpacity(layer: string, opacity: number): boolean;
    /**
     * Show or hide layers by source name or label (e.g. "Top copper").
     * Returns whether any layer matched.
     */
    setLayerVisibility(layer: string, visible: boolean): boolean;
    /**
     * View the board from the `"top"` or `"bottom"` (mirrored, stack reversed).
     */
    setSide(side: string): void;
    /**
     * Replace all layers with `sources`: a File, `{ name, content }`, `{ name?, url }`,
     * or an iterable of those (Array, FileList). Layers appear as each source finishes
     * loading. Resolves with the project summary once every source has been handled;
     * per-source failures are reported in `warnings` rather than rejecting.
     */
    setSources(sources: any): Promise<any>;
    /**
     * Current viewing side: `"top"` or `"bottom"`.
     */
    side(): string;
}

/**
 * Parse sources without rendering. Resolves with `{ layers, bbox, warnings, skipped }`, where
 * each layer has `name`, `function`, `side`, `inner`, `drawings`, `clear_drawings`,
 * and `bbox`. Coordinates are millimetres with Y pointing down.
 */
export function parseSources(sources: any): Promise<any>;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_gerberviewer_free: (a: number, b: number) => void;
    readonly gerberviewer_addSources: (a: number, b: any) => any;
    readonly gerberviewer_clear: (a: number) => void;
    readonly gerberviewer_destroy: (a: number) => void;
    readonly gerberviewer_exportProject: (a: number) => [number, number, number, number];
    readonly gerberviewer_fit: (a: number) => void;
    readonly gerberviewer_mount: (a: number, b: any) => [number, number];
    readonly gerberviewer_new: (a: any) => [number, number, number];
    readonly gerberviewer_onChange: (a: number, b: number) => void;
    readonly gerberviewer_project: (a: number) => [number, number, number];
    readonly gerberviewer_resize: (a: number) => void;
    readonly gerberviewer_setLayerColor: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly gerberviewer_setLayerOpacity: (a: number, b: number, c: number, d: number) => number;
    readonly gerberviewer_setLayerVisibility: (a: number, b: number, c: number, d: number) => number;
    readonly gerberviewer_setSide: (a: number, b: number, c: number) => [number, number];
    readonly gerberviewer_setSources: (a: number, b: any) => any;
    readonly gerberviewer_side: (a: number) => [number, number];
    readonly parseSources: (a: any) => any;
    readonly wasm_bindgen_2a4536ab01696c93___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut__web_sys_a78a682f07cb1009___features__gen_Event__Event____Output_______: (a: number, b: number) => void;
    readonly wasm_bindgen_2a4536ab01696c93___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut__wasm_bindgen_2a4536ab01696c93___JsValue____Output___core_ed718c3d60ebd546___result__Result_____wasm_bindgen_2a4536ab01696c93___JsError___: (a: number, b: number) => void;
    readonly wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___wasm_bindgen_2a4536ab01696c93___JsValue__core_ed718c3d60ebd546___result__Result_____wasm_bindgen_2a4536ab01696c93___JsError__: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined______: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___web_sys_a78a682f07cb1009___features__gen_Event__Event_____: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke______: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
