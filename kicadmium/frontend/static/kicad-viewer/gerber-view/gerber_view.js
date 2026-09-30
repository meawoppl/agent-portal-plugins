/* @ts-self-types="./gerber_view.d.ts" */

/**
 * Embeddable Gerber viewer. Create one, `mount` it into an element with a size,
 * then give it sources with `setSources`.
 */
export class GerberViewer {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        GerberViewerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_gerberviewer_free(ptr, 0);
    }
    /**
     * Add `sources` to the current layers. Accepts the same forms as `setSources`.
     * @param {any} sources
     * @returns {Promise<any>}
     */
    addSources(sources) {
        const ret = wasm.gerberviewer_addSources(this.__wbg_ptr, sources);
        return ret;
    }
    /**
     * Remove all layers and diagnostics. Pending loads from earlier calls are discarded.
     */
    clear() {
        wasm.gerberviewer_clear(this.__wbg_ptr);
    }
    /**
     * Unmount from the DOM and drop all layers. Call `free()` afterwards to release
     * the WebAssembly memory held by this object.
     */
    destroy() {
        wasm.gerberviewer_destroy(this.__wbg_ptr);
    }
    /**
     * Full parsed geometry as a JSON string, in the same shape `parseSources` returns.
     * @returns {string}
     */
    exportProject() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.gerberviewer_exportProject(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Fit the board in the viewport. The view keeps refitting on resize and new
     * layers until the user pans or zooms.
     */
    fit() {
        wasm.gerberviewer_fit(this.__wbg_ptr);
    }
    /**
     * Render into `container`, which should have a non-zero size. The viewer fills
     * it and tracks its size. Mounting again moves the viewer to the new container.
     * @param {HTMLElement} container
     */
    mount(container) {
        const ret = wasm.gerberviewer_mount(this.__wbg_ptr, container);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Options (all optional): `{ background: "#12161c", controls: false, padding: 16 }`.
     * @param {any} options
     */
    constructor(options) {
        const ret = wasm.gerberviewer_new(options);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0] >>> 0;
        GerberViewerFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Register a callback invoked with the project summary whenever layers, styles,
     * or the viewing side change. Pass `null` to remove it.
     * @param {Function | null} [callback]
     */
    onChange(callback) {
        wasm.gerberviewer_onChange(this.__wbg_ptr, isLikeNone(callback) ? 0 : addToExternrefTable0(callback));
    }
    /**
     * Summary of the loaded project: layers (name, label, function, side, colour,
     * visibility, counts, bbox), board bbox in millimetres, `warnings` (problems),
     * `skipped` (non-fabrication or empty files), and view side.
     * @returns {any}
     */
    project() {
        const ret = wasm.gerberviewer_project(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Re-read the container size. Only needed where `ResizeObserver` is unavailable
     * and the container changes size without a window resize.
     */
    resize() {
        wasm.gerberviewer_resize(this.__wbg_ptr);
    }
    /**
     * Set a layer's colour (any CSS colour) by source name or label.
     * @param {string} layer
     * @param {string} color
     * @returns {boolean}
     */
    setLayerColor(layer, color) {
        const ptr0 = passStringToWasm0(layer, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(color, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.gerberviewer_setLayerColor(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return ret !== 0;
    }
    /**
     * Set a layer's opacity (0 to 1) by source name or label.
     * @param {string} layer
     * @param {number} opacity
     * @returns {boolean}
     */
    setLayerOpacity(layer, opacity) {
        const ptr0 = passStringToWasm0(layer, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.gerberviewer_setLayerOpacity(this.__wbg_ptr, ptr0, len0, opacity);
        return ret !== 0;
    }
    /**
     * Show or hide layers by source name or label (e.g. "Top copper").
     * Returns whether any layer matched.
     * @param {string} layer
     * @param {boolean} visible
     * @returns {boolean}
     */
    setLayerVisibility(layer, visible) {
        const ptr0 = passStringToWasm0(layer, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.gerberviewer_setLayerVisibility(this.__wbg_ptr, ptr0, len0, visible);
        return ret !== 0;
    }
    /**
     * View the board from the `"top"` or `"bottom"` (mirrored, stack reversed).
     * @param {string} side
     */
    setSide(side) {
        const ptr0 = passStringToWasm0(side, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.gerberviewer_setSide(this.__wbg_ptr, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Replace all layers with `sources`: a File, `{ name, content }`, `{ name?, url }`,
     * or an iterable of those (Array, FileList). Layers appear as each source finishes
     * loading. Resolves with the project summary once every source has been handled;
     * per-source failures are reported in `warnings` rather than rejecting.
     * @param {any} sources
     * @returns {Promise<any>}
     */
    setSources(sources) {
        const ret = wasm.gerberviewer_setSources(this.__wbg_ptr, sources);
        return ret;
    }
    /**
     * Current viewing side: `"top"` or `"bottom"`.
     * @returns {string}
     */
    side() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.gerberviewer_side(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
}
if (Symbol.dispose) GerberViewer.prototype[Symbol.dispose] = GerberViewer.prototype.free;

/**
 * Parse sources without rendering. Resolves with `{ layers, bbox, warnings, skipped }`, where
 * each layer has `name`, `function`, `side`, `inner`, `drawings`, `clear_drawings`,
 * and `bbox`. Coordinates are millimetres with Y pointing down.
 * @param {any} sources
 * @returns {Promise<any>}
 */
export function parseSources(sources) {
    const ret = wasm.parseSources(sources);
    return ret;
}

function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_25ba92dc73c79ff8: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg___wbindgen_boolean_get_6220f2dea0de122f: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_46569e04243a7370: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_9496351ef250d988: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_function_c2cd65fa64fb6f48: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_bff54fa0dc7087b7: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_8bdde3c0d4cfc731: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_undefined_93ede8eff94d1589: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_288868c9456dc316: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_30753c95eb065096: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_e935d22db323c682: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_f1861aae416df39d: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_5fc2671fecbcd36f: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_addEventListener_62bc32f54ebddb7b: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.addEventListener(getStringFromWasm0(arg1, arg2), arg3, arg4);
        }, arguments); },
        __wbg_all_520c3c3db349e3a3: function(arg0) {
            const ret = Promise.all(arg0);
            return ret;
        },
        __wbg_appendChild_fccfe6534fb0b5a0: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.appendChild(arg1);
            return ret;
        }, arguments); },
        __wbg_apply_fbd299a9bee81798: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.apply(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_arrayBuffer_34604df7f6c8c76d: function() { return handleError(function (arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        }, arguments); },
        __wbg_arrayBuffer_f05391f87ca6d4e2: function(arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        },
        __wbg_button_191f7600724fb244: function(arg0) {
            const ret = arg0.button;
            return ret;
        },
        __wbg_call_7d28fcf67f55a9a5: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_b3f19a8e2e3a36bd: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_cancelAnimationFrame_ceee9ed307f40b97: function() { return handleError(function (arg0, arg1) {
            arg0.cancelAnimationFrame(arg1);
        }, arguments); },
        __wbg_checked_2dd347d7c88bc521: function(arg0) {
            const ret = arg0.checked;
            return ret;
        },
        __wbg_clearRect_1f896fdd16208909: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.clearRect(arg1, arg2, arg3, arg4);
        },
        __wbg_clearTimeout_253531f7ca610704: function(arg0, arg1) {
            arg0.clearTimeout(arg1);
        },
        __wbg_clientHeight_9a1feb9c4ea26920: function(arg0) {
            const ret = arg0.clientHeight;
            return ret;
        },
        __wbg_clientWidth_7d68cdd2b9206ff0: function(arg0) {
            const ret = arg0.clientWidth;
            return ret;
        },
        __wbg_clientX_36f704a7cb093523: function(arg0) {
            const ret = arg0.clientX;
            return ret;
        },
        __wbg_clientY_cba16e090376fb35: function(arg0) {
            const ret = arg0.clientY;
            return ret;
        },
        __wbg_closest_af69795e9920cf6d: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.closest(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_construct_e4cfacb1d728ddc3: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.construct(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_createElement_804bec186e141f2c: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.createElement(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_ctrlKey_ba267200710ffe65: function(arg0) {
            const ret = arg0.ctrlKey;
            return ret;
        },
        __wbg_deltaMode_9d02a5420a260646: function(arg0) {
            const ret = arg0.deltaMode;
            return ret;
        },
        __wbg_deltaX_4bcdc6e4c4775168: function(arg0) {
            const ret = arg0.deltaX;
            return ret;
        },
        __wbg_deltaY_8e19329254ab7c79: function(arg0) {
            const ret = arg0.deltaY;
            return ret;
        },
        __wbg_devicePixelRatio_007252f1a5a53f2f: function(arg0) {
            const ret = arg0.devicePixelRatio;
            return ret;
        },
        __wbg_done_374b57982012b5d2: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_drawImage_3120a11baf2c59c6: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawImage(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_drawImage_5221d504643da417: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.drawImage(arg1, arg2, arg3);
        }, arguments); },
        __wbg_error_73e3cc54de9fc42f: function(arg0, arg1) {
            console.error(arg0, arg1);
        },
        __wbg_fetch_aeb7d7ec4ad8a935: function(arg0, arg1, arg2) {
            const ret = arg0.fetch(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_fillRect_d5312677add9a3c0: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.fillRect(arg1, arg2, arg3, arg4);
        },
        __wbg_fill_67357a9628d44e53: function(arg0, arg1, arg2) {
            arg0.fill(arg1, __wbindgen_enum_CanvasWindingRule[arg2]);
        },
        __wbg_getAttribute_4c8c3b86661e08bd: function(arg0, arg1, arg2, arg3) {
            const ret = arg1.getAttribute(getStringFromWasm0(arg2, arg3));
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_getContext_c7cdd24f04542db1: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_get_06880b4164c9d56f: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_4eef9e87f0ce4610: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_with_ref_key_6412cf3094599694: function(arg0, arg1) {
            const ret = arg0[arg1];
            return ret;
        },
        __wbg_height_7ab61ce526cb2863: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_95c14839b9fd2ebd: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Blob_b9d0ca68182a1546: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Blob;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_CanvasRenderingContext2d_ee766cbe6e965652: function(arg0) {
            let result;
            try {
                result = arg0 instanceof CanvasRenderingContext2D;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Element_bf74f38bb904e93e: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Element;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Error_11a727b99211c658: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_File_6ecbdda02689bc6a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof File;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlCanvasElement_70b9f3937a5713de: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLCanvasElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlElement_d77543da723e198a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_HtmlInputElement_d84ad35c5a0e2efa: function(arg0) {
            let result;
            try {
                result = arg0 instanceof HTMLInputElement;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Response_e1a6a5ef7523fc42: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Response;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_3275375eda5f2bac: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_fee0139dabd59cb2: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isView_3e0e48e73f94066a: function(arg0) {
            const ret = ArrayBuffer.isView(arg0);
            return ret;
        },
        __wbg_iterator_7051ec21ea3d830d: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_length_9f99fa74b0f6a829: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_message_4a29b473ab9fc137: function(arg0) {
            const ret = arg0.message;
            return ret;
        },
        __wbg_metaKey_77d27e65f66ae5fb: function(arg0) {
            const ret = arg0.metaKey;
            return ret;
        },
        __wbg_name_a4fb3d548c173b86: function(arg0, arg1) {
            const ret = arg1.name;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_new_0303a5efc9d0666b: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_1b4edd8807f9a802: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_dcf435def74b2b31: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_typed_2895616ed377a944: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_typed_ba4e97687832c766: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined______(state0.a, state0.b, arg0, arg1);
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = state0.b = 0;
            }
        },
        __wbg_new_with_path_string_40f322f16edb5d25: function() { return handleError(function (arg0, arg1) {
            const ret = new Path2D(getStringFromWasm0(arg0, arg1));
            return ret;
        }, arguments); },
        __wbg_next_86766281e22c68f4: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_next_dc5b11ade31a2e28: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_now_1433445baf3c0966: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_of_b18d3d87247c334f: function(arg0) {
            const ret = Array.of(arg0);
            return ret;
        },
        __wbg_offsetX_b7f1dfa9ec5cea11: function(arg0) {
            const ret = arg0.offsetX;
            return ret;
        },
        __wbg_offsetY_89c1f65d0754c64c: function(arg0) {
            const ret = arg0.offsetY;
            return ret;
        },
        __wbg_ok_9199c7844de8e0eb: function(arg0) {
            const ret = arg0.ok;
            return ret;
        },
        __wbg_ownerDocument_3403aac2fd929b5d: function(arg0) {
            const ret = arg0.ownerDocument;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_pointerId_a3b7ef464b4a4558: function(arg0) {
            const ret = arg0.pointerId;
            return ret;
        },
        __wbg_preventDefault_b720bf341c12f139: function(arg0) {
            arg0.preventDefault();
        },
        __wbg_prototypesetcall_8d4e3621ad98652b: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_push_1cf379a741873bf6: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_queueMicrotask_a5feb489fe8e411d: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_fc670f89e049713c: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_reject_d22bd1fe62965a7e: function(arg0) {
            const ret = Promise.reject(arg0);
            return ret;
        },
        __wbg_removeEventListener_0ea51e33abdfb800: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            arg0.removeEventListener(getStringFromWasm0(arg1, arg2), arg3);
        }, arguments); },
        __wbg_remove_87f07f4e98e74bb3: function(arg0) {
            arg0.remove();
        },
        __wbg_requestAnimationFrame_71a3ab2fdc057afd: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.requestAnimationFrame(arg1);
            return ret;
        }, arguments); },
        __wbg_resolve_658ba2d8c263805a: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_setPointerCapture_b002686eacb8ed2c: function() { return handleError(function (arg0, arg1) {
            arg0.setPointerCapture(arg1);
        }, arguments); },
        __wbg_setProperty_013fa976add474bc: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.setProperty(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_setTimeout_98ce3059ee895904: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.setTimeout(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_setTransform_b23dc181f62f2ab4: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setTransform(arg1, arg2, arg3, arg4, arg5, arg6);
        }, arguments); },
        __wbg_set_58d19822a2826a42: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_className_87fce45e4f76dda8: function(arg0, arg1, arg2) {
            arg0.className = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_cssText_ab5f1565d722bb21: function(arg0, arg1, arg2) {
            arg0.cssText = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_fillStyle_bf742d7f402b3082: function(arg0, arg1, arg2) {
            arg0.fillStyle = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_globalAlpha_1583153384f13f70: function(arg0, arg1) {
            arg0.globalAlpha = arg1;
        },
        __wbg_set_globalCompositeOperation_8802092eb0c677a8: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.globalCompositeOperation = getStringFromWasm0(arg1, arg2);
        }, arguments); },
        __wbg_set_height_3a6def819aed7f92: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_innerHTML_2a9e52f8ac95965f: function(arg0, arg1, arg2) {
            arg0.innerHTML = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_lineCap_635509a27babbb14: function(arg0, arg1, arg2) {
            arg0.lineCap = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_lineJoin_086209bd1f3f7c27: function(arg0, arg1, arg2) {
            arg0.lineJoin = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_lineWidth_aabfc8cf7aeaade6: function(arg0, arg1) {
            arg0.lineWidth = arg1;
        },
        __wbg_set_passive_3617c1d2a60e25c1: function(arg0, arg1) {
            arg0.passive = arg1 !== 0;
        },
        __wbg_set_strokeStyle_c2af771348ced095: function(arg0, arg1, arg2) {
            arg0.strokeStyle = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_width_c9a28633ba15c744: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_static_accessor_GLOBAL_88146d31754465df: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_fb1dffa7d2fb9578: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_3ebcb3e5cc31b577: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_6370bd11a8e879db: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_status_e5f6872b7af3b063: function(arg0) {
            const ret = arg0.status;
            return ret;
        },
        __wbg_stroke_53ab7f5289d87498: function(arg0, arg1) {
            arg0.stroke(arg1);
        },
        __wbg_style_883e6e5a4c744b0c: function(arg0) {
            const ret = arg0.style;
            return ret;
        },
        __wbg_target_7d97e2abdad81323: function(arg0) {
            const ret = arg0.target;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_then_3812e2b326ecb129: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_a48d77907d9dd003: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_value_2f527089a16a5fae: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_width_4e6cb52771d12542: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 59, function: Function { arguments: [NamedExternref("Event")], shim_idx: 60, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen_2a4536ab01696c93___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut__web_sys_a78a682f07cb1009___features__gen_Event__Event____Output_______, wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___web_sys_a78a682f07cb1009___features__gen_Event__Event_____);
            return ret;
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 59, function: Function { arguments: [], shim_idx: 62, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen_2a4536ab01696c93___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut__web_sys_a78a682f07cb1009___features__gen_Event__Event____Output_______, wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke______);
            return ret;
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { dtor_idx: 89, function: Function { arguments: [Externref], shim_idx: 90, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm.wasm_bindgen_2a4536ab01696c93___closure__destroy___dyn_core_ed718c3d60ebd546___ops__function__FnMut__wasm_bindgen_2a4536ab01696c93___JsValue____Output___core_ed718c3d60ebd546___result__Result_____wasm_bindgen_2a4536ab01696c93___JsError___, wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___wasm_bindgen_2a4536ab01696c93___JsValue__core_ed718c3d60ebd546___result__Result_____wasm_bindgen_2a4536ab01696c93___JsError__);
            return ret;
        },
        __wbindgen_cast_0000000000000004: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_cast_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_cast_0000000000000006: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./gerber_view_bg.js": import0,
    };
}

function wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke______(arg0, arg1) {
    wasm.wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke______(arg0, arg1);
}

function wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___web_sys_a78a682f07cb1009___features__gen_Event__Event_____(arg0, arg1, arg2) {
    wasm.wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___web_sys_a78a682f07cb1009___features__gen_Event__Event_____(arg0, arg1, arg2);
}

function wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___wasm_bindgen_2a4536ab01696c93___JsValue__core_ed718c3d60ebd546___result__Result_____wasm_bindgen_2a4536ab01696c93___JsError__(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___wasm_bindgen_2a4536ab01696c93___JsValue__core_ed718c3d60ebd546___result__Result_____wasm_bindgen_2a4536ab01696c93___JsError__(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined______(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen_2a4536ab01696c93___convert__closures_____invoke___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined___js_sys_e0a471bac044a5ed___Function_fn_wasm_bindgen_2a4536ab01696c93___JsValue_____wasm_bindgen_2a4536ab01696c93___sys__Undefined______(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_CanvasWindingRule = ["nonzero", "evenodd"];
const GerberViewerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_gerberviewer_free(ptr >>> 0, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => state.dtor(state.a, state.b));

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, dtor, f) {
    const state = { a: arg0, b: arg1, cnt: 1, dtor };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            state.dtor(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('gerber_view_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
