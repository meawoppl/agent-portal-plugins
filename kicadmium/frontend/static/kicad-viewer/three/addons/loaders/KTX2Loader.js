// kicadmium stub: KiCad GLB exports carry no KHR_texture_basisu textures, so
// the Basis/KTX2 transcoder is not shipped. Using it is an error, not a fetch.
export class KTX2Loader {
  setTranscoderPath() { return this; }
  detectSupport() { return this; }
  load(_url, _onLoad, _onProgress, onError) {
    const err = new Error("KTX2 textures are not supported in kicadmium (KiCad GLBs do not use them)");
    if (onError) onError(err); else throw err;
  }
  dispose() {}
}
