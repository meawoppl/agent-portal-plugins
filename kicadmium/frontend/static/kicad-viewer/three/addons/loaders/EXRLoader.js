// kicadmium stub: the viewer uses the built-in neutral RoomEnvironment; remote
// EXR environment maps are not fetched or decoded.
export class EXRLoader {
  load(_url, _onLoad, _onProgress, onError) {
    const err = new Error("EXR environment maps are not supported in kicadmium; use the neutral environment");
    if (onError) onError(err); else throw err;
  }
}
