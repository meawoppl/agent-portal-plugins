// kicadmium stub: KiCad GLB exports do not use EXT_meshopt_compression.
export const MeshoptDecoder = {
  supported: false,
  ready: Promise.resolve(),
  decodeGltfBuffer() {
    throw new Error("meshopt-compressed buffers are not supported in kicadmium (KiCad GLBs do not use them)");
  },
};
