module linkage

# A base and a link joined by a revolute mate. The datums travel with each
# part as @meta(assembly.datums=...) and show as markers in the pane.

@meta(assembly.datums=[{
    "id": "pivot",
    "kind": "axis",
    "origin_mm": [12.5, -7.0, 4.0],
    "direction": [0.0, 0.0, 1.0],
}])
command BASE() -> solid:
    emit box(30.0, 16.0, 8.0)

@meta(assembly.datums=[{
    "id": "pivot",
    "kind": "axis",
    "origin_mm": [-12.0, 0.0, -1.5],
    "direction": [0.0, 0.0, 1.0],
}])
command LINK() -> solid:
    let bar: solid = box(30.0, 5.0, 3.0)
    let eye: solid = translate(cylinder(1.5, 5.0), -12.0, 0.0, -2.5)
    emit difference(bar, eye)

command mechanism(angle_deg: float) -> assembly:
    let asm: assembly = assembly("linkage")
    add_part(asm, BASE(), "base")
    add_part(asm, LINK(), "link")
    add_named_mate(asm, "pivot", "revolute", "base", "pivot", "link", "pivot")
    set_mate_limits(asm, "pivot", radians(-120.0), radians(120.0))
    solve_assembly(asm, "base")
    set_joint_position(asm, "pivot", radians(angle_deg))
    emit asm

# Posed geometry of every part.
command POSED(angle_deg: float @ui(widget="slider", label="Joint angle", min=-120.0, max=120.0, step=5.0) = 30.0) -> solid:
    emit assembly_compound(mechanism(angle_deg))

# The semantic mate/part graph as JSON (saved as assembly.json in the run).
command GRAPH(angle_deg: float = 30.0) -> string:
    emit emit_assembly(mechanism(angle_deg))
