// kicadmium stub: KiCad GLB exports are not Draco-compressed, so the Draco
// decoder (and its remote unpkg fetch) is not shipped.
export class DRACOLoader {
  setDecoderPath() { return this; }
  setDecoderConfig() { return this; }
  setWorkerLimit() { return this; }
  preload() { return this; }
  decodeDracoFile(_buffer, _callback, _attributeIDs, _attributeTypes, _vertexColorSpace, onError) {
    const err = new Error("Draco-compressed meshes are not supported in kicadmium (KiCad GLBs do not use them)");
    if (onError) onError(err); else throw err;
  }
  dispose() {}
}
