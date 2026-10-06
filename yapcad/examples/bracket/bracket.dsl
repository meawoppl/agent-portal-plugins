module bracket

# A mounting plate with a bore and two bolt holes. Builds in a few seconds
# with the pure-Python mesh engine; use representation "sdf" for a filleted
# field-built variant (slower).
command PLATE(
    width: float @ui(widget="slider", label="Width", min=20.0, max=120.0, step=1.0) = 60.0,
    depth: float @ui(widget="slider", label="Depth", min=15.0, max=80.0, step=1.0) = 30.0,
    thickness: float @ui(label="Thickness", min=2.0, max=20.0, group="Stock") = 6.0,
    bore: float @ui(label="Bore diameter", min=2.0, max=30.0, group="Holes") = 10.0,
    bolt: float @ui(label="Bolt clearance", snap="metric_tap", group="Holes") = 4.5
) -> solid:
    require width > bore + 4.0 * bolt
    require thickness > 0.0
    let plate: solid = box(width, depth, thickness)
    let cut: float = thickness + 2.0
    let hole: solid = translate(cylinder(bore / 2.0, cut), 0.0, 0.0, -cut / 2.0)
    let left: solid = translate(cylinder(bolt / 2.0, cut),
                                -width / 2.0 + 2.0 * bolt, 0.0, -cut / 2.0)
    let right: solid = translate(cylinder(bolt / 2.0, cut),
                                 width / 2.0 - 2.0 * bolt, 0.0, -cut / 2.0)
    emit difference(plate, hole, left, right)

# The plate outline as 2D geometry, for laser or waterjet DXF/SVG export.
command PROFILE(width: float = 60.0, depth: float = 30.0, bore: float = 10.0) -> region2d:
    let outline: region2d = rectangle(width, depth)
    let opening: region2d = disk(point(0.0, 0.0), bore / 2.0)
    emit difference2d(outline, opening)

# A derived value: plate mass in grams for 6061 aluminium.
command MASS_G(width: float = 60.0, depth: float = 30.0, thickness: float = 6.0) -> float:
    emit width * depth * thickness * 0.0027
