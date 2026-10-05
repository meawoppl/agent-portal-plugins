//! Rust/WASM lifecycle and UI for the vendored Three.js model renderer.
//!
//! Three.js stays JavaScript; the application-specific loading, camera,
//! lifecycle, resize and error handling are native Rust.

use std::{cell::RefCell, rc::Rc};

use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{closure::Closure, prelude::*, JsCast};
use web_sys::HtmlCanvasElement;
use yew::prelude::*;

type AnimationSlot = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
type LoadSlot = Rc<RefCell<Option<Closure<dyn FnMut(JsValue)>>>>;

#[wasm_bindgen(raw_module = "three")]
extern "C" {
    #[wasm_bindgen(js_name = Scene)]
    type Scene;
    #[wasm_bindgen(constructor, js_class = Scene)]
    fn new_scene() -> Scene;
    #[wasm_bindgen(method)]
    fn add(this: &Scene, object: &JsValue);

    #[wasm_bindgen(js_name = PerspectiveCamera)]
    type Camera;
    #[wasm_bindgen(constructor, js_class = PerspectiveCamera)]
    fn new_camera(fov: f64, aspect: f64, near: f64, far: f64) -> Camera;
    #[wasm_bindgen(method, js_name = updateProjectionMatrix)]
    fn update_projection(this: &Camera);

    #[wasm_bindgen(js_name = WebGLRenderer)]
    type Renderer;
    #[wasm_bindgen(constructor, js_class = WebGLRenderer)]
    fn new_renderer(options: &JsValue) -> Renderer;
    #[wasm_bindgen(method, js_name = setPixelRatio)]
    fn set_pixel_ratio(this: &Renderer, ratio: f64);
    #[wasm_bindgen(method, js_name = setSize)]
    fn set_size(this: &Renderer, width: f64, height: f64, update_style: bool);
    #[wasm_bindgen(method)]
    fn render(this: &Renderer, scene: &Scene, camera: &Camera);
    #[wasm_bindgen(method)]
    fn dispose(this: &Renderer);

    #[wasm_bindgen(js_name = AmbientLight)]
    type AmbientLight;
    #[wasm_bindgen(constructor, js_class = AmbientLight)]
    fn new_ambient(color: u32, intensity: f64) -> AmbientLight;
    #[wasm_bindgen(js_name = DirectionalLight)]
    type DirectionalLight;
    #[wasm_bindgen(constructor, js_class = DirectionalLight)]
    fn new_directional(color: u32, intensity: f64) -> DirectionalLight;

    #[wasm_bindgen(js_name = Box3)]
    type Box3;
    #[wasm_bindgen(constructor, js_class = Box3)]
    fn new_box() -> Box3;
    #[wasm_bindgen(method, js_name = setFromObject)]
    fn set_from_object(this: &Box3, object: &JsValue) -> Box3;
    #[wasm_bindgen(method, js_name = getSize)]
    fn get_size(this: &Box3, target: &Vec3) -> Vec3;
    #[wasm_bindgen(method, js_name = getCenter)]
    fn get_center(this: &Box3, target: &Vec3) -> Vec3;

    #[wasm_bindgen(js_name = Vector3)]
    type Vec3;
    #[wasm_bindgen(constructor, js_class = Vector3)]
    fn new_vec3(x: f64, y: f64, z: f64) -> Vec3;
    #[wasm_bindgen(method)]
    fn set(this: &Vec3, x: f64, y: f64, z: f64) -> Vec3;
    #[wasm_bindgen(method, getter)]
    fn x(this: &Vec3) -> f64;
    #[wasm_bindgen(method, getter)]
    fn y(this: &Vec3) -> f64;
    #[wasm_bindgen(method, getter)]
    fn z(this: &Vec3) -> f64;
}

#[wasm_bindgen(raw_module = "three/addons/loaders/GLTFLoader.js")]
extern "C" {
    #[wasm_bindgen(js_name = GLTFLoader)]
    type GltfLoader;
    #[wasm_bindgen(constructor, js_class = GLTFLoader)]
    fn new_loader() -> GltfLoader;
}

#[wasm_bindgen(raw_module = "three/addons/controls/OrbitControls.js")]
extern "C" {
    #[wasm_bindgen(js_name = OrbitControls)]
    type OrbitControls;
    #[wasm_bindgen(constructor, js_class = OrbitControls)]
    fn new_controls(camera: &Camera, canvas: &HtmlCanvasElement) -> OrbitControls;
    #[wasm_bindgen(method)]
    fn update(this: &OrbitControls);
    #[wasm_bindgen(method)]
    fn dispose(this: &OrbitControls);
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub project: AttrValue,
    pub revision: AttrValue,
    /// Explicit GLB endpoint for library/model previews. The board viewer
    /// derives its revision-pinned endpoint when this is absent.
    #[prop_or_default]
    pub url: Option<AttrValue>,
    #[prop_or(true)]
    pub active: bool,
}

struct Runtime {
    renderer: Rc<Renderer>,
    controls: Rc<OrbitControls>,
    animation: AnimationSlot,
    animation_id: Rc<RefCell<Option<i32>>>,
    resize: Closure<dyn FnMut()>,
    loaded: LoadSlot,
    failed: LoadSlot,
    model: Rc<RefCell<Option<JsValue>>>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            if let Some(id) = self.animation_id.borrow_mut().take() {
                let _ = window.cancel_animation_frame(id);
            }
            let _ = window.remove_event_listener_with_callback(
                "resize",
                self.resize.as_ref().unchecked_ref(),
            );
        }
        self.animation.borrow_mut().take();
        self.loaded.borrow_mut().take();
        self.failed.borrow_mut().take();
        if let Some(model) = self.model.borrow_mut().take() {
            dispose_model(&model);
        }
        self.controls.dispose();
        self.renderer.dispose();
    }
}

fn call_no_args(object: &JsValue, method: &str) {
    if let Ok(value) = Reflect::get(object, &method.into()) {
        if let Ok(function) = value.dyn_into::<Function>() {
            let _ = function.call0(object);
        }
    }
}

/// Release GPU-backed geometry, material, and texture allocations retained by
/// Three.js. `WebGLRenderer::dispose` alone intentionally does not own these.
fn dispose_model(model: &JsValue) {
    let visitor = Closure::<dyn FnMut(JsValue)>::new(move |object: JsValue| {
        if let Ok(geometry) = Reflect::get(&object, &"geometry".into()) {
            if !geometry.is_undefined() {
                call_no_args(&geometry, "dispose");
            }
        }
        if let Ok(material) = Reflect::get(&object, &"material".into()) {
            let materials = if js_sys::Array::is_array(&material) {
                js_sys::Array::from(&material)
            } else {
                js_sys::Array::of1(&material)
            };
            for value in materials.iter() {
                if value.is_null() || value.is_undefined() {
                    continue;
                }
                // Dispose textures reachable from enumerable material fields.
                for key in Object::keys(value.unchecked_ref::<Object>()) {
                    if let Ok(candidate) = Reflect::get(&value, &key) {
                        if Reflect::get(&candidate, &"isTexture".into())
                            .ok()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            call_no_args(&candidate, "dispose");
                        }
                    }
                }
                call_no_args(&value, "dispose");
            }
        }
    });
    if let Ok(value) = Reflect::get(model, &"traverse".into()) {
        if let Ok(traverse) = value.dyn_into::<Function>() {
            let _ = traverse.call1(model, visitor.as_ref().unchecked_ref());
        }
    }
}

fn property<T: JsCast>(object: &JsValue, name: &str) -> Option<T> {
    Reflect::get(object, &name.into()).ok()?.dyn_into().ok()
}

fn number(object: &JsValue, name: &str, value: f64) {
    let _ = Reflect::set(object, &name.into(), &value.into());
}

fn fit_distance(width: f64, height: f64, depth: f64, vertical_fov: f64, aspect: f64) -> f64 {
    // Fit the bounding sphere against whichever viewport dimension has the
    // tighter field of view. Unlike an arbitrary world-unit floor, this works
    // for KiCad GLBs, whose coordinates are metres (a normal PCB spans ~0.1).
    let radius = 0.5 * (width * width + height * height + depth * depth).sqrt();
    let vertical_half = 0.5 * vertical_fov.to_radians();
    let horizontal_half = (vertical_half.tan() * aspect.max(0.01)).atan();
    radius / vertical_half.min(horizontal_half).sin().max(1.0e-6) * 1.12
}

/// Depth range for an orbiting camera around a model with `radius`.
///
/// The old near plane was derived from the initial fitted camera distance.
/// It therefore sat more than a radius in front of the camera and cut through
/// the board as soon as OrbitControls zoomed closer.  Keep the near plane near
/// the camera instead, and make the far plane cover the controls' complete
/// zoom-out range.  Scaling both by the model radius keeps this useful for
/// metre-scale board GLBs and millimetre-scale library previews alike.
fn clip_planes(radius: f64, fitted_distance: f64) -> (f64, f64) {
    let radius = radius.max(1.0e-9);
    let near = radius / 1_000.0;
    let far = fitted_distance * 50.0 + radius * 2.0;
    (near, far.max(near * 10.0))
}

fn resize(renderer: &Renderer, camera: &Camera, canvas: &HtmlCanvasElement) {
    let width = f64::from(canvas.client_width().max(1));
    let height = f64::from(canvas.client_height().max(1));
    number(camera.as_ref(), "aspect", width / height);
    camera.update_projection();
    renderer.set_size(width, height, false);
}

fn frame(camera: &Camera, controls: &OrbitControls, model: &JsValue) {
    // setFromObject walks only this GLTF scene. Lights and helper objects from
    // our outer scene therefore cannot inflate the fitted bounds.
    let bounds = Box3::new_box().set_from_object(model);
    let size = bounds.get_size(&Vec3::new_vec3(0.0, 0.0, 0.0));
    let center = bounds.get_center(&Vec3::new_vec3(0.0, 0.0, 0.0));
    let radius = 0.5
        * (size.x() * size.x() + size.y() * size.y() + size.z() * size.z())
            .sqrt()
            .max(1.0e-9);
    let aspect = Reflect::get(camera.as_ref(), &"aspect".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let distance = fit_distance(size.x(), size.y(), size.z(), 42.0, aspect);
    // A stable oblique view shows both faces and component height while the
    // bounding-sphere distance guarantees every orientation remains framed.
    let direction = (0.65_f64, 0.4_f64, 0.65_f64);
    let direction_length =
        (direction.0 * direction.0 + direction.1 * direction.1 + direction.2 * direction.2).sqrt();
    if let Some(position) = property::<Vec3>(camera.as_ref(), "position") {
        position.set(
            center.x() + distance * direction.0 / direction_length,
            center.y() + distance * direction.1 / direction_length,
            center.z() + distance * direction.2 / direction_length,
        );
    }
    if let Some(target) = property::<Vec3>(controls.as_ref(), "target") {
        target.set(center.x(), center.y(), center.z());
    }
    let (near, far) = clip_planes(radius, distance);
    number(camera.as_ref(), "near", near);
    number(camera.as_ref(), "far", far);
    number(controls.as_ref(), "minDistance", radius * 0.05);
    number(controls.as_ref(), "maxDistance", distance * 50.0);
    camera.update_projection();
    controls.update();
}

#[cfg(test)]
mod tests {
    use super::{clip_planes, fit_distance};

    #[test]
    fn fits_metre_scale_kicad_board_without_world_unit_floor() {
        let distance = fit_distance(0.1253, 0.00355, 0.14995, 42.0, 16.0 / 9.0);
        assert!(distance > 0.2 && distance < 0.4, "distance={distance}");
    }

    #[test]
    fn portrait_view_needs_more_distance() {
        let landscape = fit_distance(0.1, 0.01, 0.05, 42.0, 16.0 / 9.0);
        let portrait = fit_distance(0.1, 0.01, 0.05, 42.0, 9.0 / 16.0);
        assert!(portrait > landscape);
    }

    #[test]
    fn clipping_plane_stays_close_when_orbiting_into_the_model() {
        let radius = 0.1;
        let distance = fit_distance(radius * 2.0, 0.01, radius * 2.0, 42.0, 16.0 / 9.0);
        let (near, far) = clip_planes(radius, distance);

        // OrbitControls permits a distance of radius * 0.05.  The near plane
        // must remain comfortably closer than that instead of being tied to
        // the much farther initial fit.
        assert!(near < radius * 0.05, "near={near}");
        assert!(far > distance * 50.0, "far={far}");
    }
}

fn start(
    canvas: HtmlCanvasElement,
    url: &str,
    active: bool,
    status: UseStateHandle<String>,
) -> Result<Runtime, JsValue> {
    let options = Object::new();
    Reflect::set(&options, &"canvas".into(), canvas.as_ref())?;
    Reflect::set(&options, &"antialias".into(), &JsValue::TRUE)?;
    let renderer = Rc::new(Renderer::new_renderer(&options));
    renderer.set_pixel_ratio(web_sys::window().map_or(1.0, |w| w.device_pixel_ratio().min(2.0)));
    let clear: Function =
        Reflect::get(renderer.as_ref().as_ref(), &"setClearColor".into())?.dyn_into()?;
    clear.call2(
        renderer.as_ref().as_ref(),
        &(0x11131d_u32).into(),
        &1.0.into(),
    )?;

    let scene = Rc::new(Scene::new_scene());
    let camera = Rc::new(Camera::new_camera(42.0, 1.0, 0.01, 100_000.0));
    let controls = Rc::new(OrbitControls::new_controls(&camera, &canvas));
    let _ = Reflect::set(
        controls.as_ref().as_ref(),
        &"enableDamping".into(),
        &JsValue::TRUE,
    );
    number(controls.as_ref().as_ref(), "dampingFactor", 0.08);
    let ambient = AmbientLight::new_ambient(0xffffff, 1.8);
    scene.add(ambient.as_ref());
    let key = DirectionalLight::new_directional(0xffffff, 3.2);
    if let Some(position) = property::<Vec3>(key.as_ref(), "position") {
        position.set(1.0, 2.0, 3.0);
    }
    scene.add(key.as_ref());
    resize(&renderer, &camera, &canvas);

    let loaded = Rc::new(RefCell::new(None));
    let failed = Rc::new(RefCell::new(None));
    let model = Rc::new(RefCell::new(None));
    *loaded.borrow_mut() = Some({
        let scene = scene.clone();
        let camera = camera.clone();
        let controls = controls.clone();
        let model_slot = model.clone();
        let status = status.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |gltf: JsValue| {
            match Reflect::get(&gltf, &"scene".into()) {
                Ok(model) => {
                    scene.add(&model);
                    frame(&camera, &controls, &model);
                    *model_slot.borrow_mut() = Some(model);
                    status.set(String::new());
                }
                Err(_) => status.set("The GLB loaded without a scene".into()),
            }
        })
    });
    *failed.borrow_mut() = Some({
        let status = status.clone();
        Closure::<dyn FnMut(JsValue)>::new(move |error: JsValue| {
            status.set(format!(
                "Could not load board model: {}",
                error
                    .as_string()
                    .unwrap_or_else(|| "unknown GLB error".into())
            ));
        })
    });
    let loader = GltfLoader::new_loader();
    let load: Function = Reflect::get(loader.as_ref(), &"load".into())?.dyn_into()?;
    load.call4(
        loader.as_ref(),
        &url.into(),
        loaded.borrow().as_ref().unwrap().as_ref(),
        &JsValue::UNDEFINED,
        failed.borrow().as_ref().unwrap().as_ref(),
    )?;

    let animation: AnimationSlot = Rc::new(RefCell::new(None));
    let animation_id = Rc::new(RefCell::new(None));
    let callback_slot = animation.clone();
    let id_slot = animation_id.clone();
    let loop_renderer = renderer.clone();
    let loop_scene = scene.clone();
    let loop_camera = camera.clone();
    let loop_controls = controls.clone();
    *animation.borrow_mut() = Some(Closure::<dyn FnMut(f64)>::new(move |_| {
        if active {
            loop_controls.update();
            loop_renderer.render(&loop_scene, &loop_camera);
        }
        if let (Some(window), Some(callback)) = (web_sys::window(), callback_slot.borrow().as_ref())
        {
            *id_slot.borrow_mut() = window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .ok();
        }
    }));
    if let (Some(window), Some(callback)) = (web_sys::window(), animation.borrow().as_ref()) {
        *animation_id.borrow_mut() = window
            .request_animation_frame(callback.as_ref().unchecked_ref())
            .ok();
    }

    let rr = renderer.clone();
    let rc = camera.clone();
    let resize_canvas = canvas.clone();
    let resize_callback = Closure::<dyn FnMut()>::new(move || resize(&rr, &rc, &resize_canvas));
    if let Some(window) = web_sys::window() {
        window
            .add_event_listener_with_callback("resize", resize_callback.as_ref().unchecked_ref())?;
    }
    Ok(Runtime {
        renderer,
        controls,
        animation,
        animation_id,
        resize: resize_callback,
        loaded,
        failed,
        model,
    })
}

#[function_component(ModelView)]
pub fn model_view(props: &Props) -> Html {
    let canvas = use_node_ref();
    let status = use_state(|| "Loading board model…".to_owned());
    let runtime = use_mut_ref(|| None::<Runtime>);
    {
        let canvas = canvas.clone();
        let project = props.project.to_string();
        let revision = props.revision.to_string();
        let explicit_url = props.url.as_ref().map(ToString::to_string);
        let active = props.active;
        let status = status.clone();
        let runtime = runtime.clone();
        use_effect_with(
            (
                project.clone(),
                revision.clone(),
                explicit_url.clone(),
                active,
            ),
            move |_| {
                runtime.borrow_mut().take();
                status.set(if explicit_url.is_some() {
                    "Loading library model…".into()
                } else {
                    "Loading board model…".into()
                });
                if let Some(canvas) = canvas.cast::<HtmlCanvasElement>() {
                    let path = explicit_url.clone().unwrap_or_else(|| {
                        format!("/api/kicad/model.glb?rev={}", crate::api::encode(&revision))
                    });
                    match start(
                        canvas,
                        &crate::api::url(&path, &project),
                        active,
                        status.clone(),
                    ) {
                        Ok(next) => *runtime.borrow_mut() = Some(next),
                        Err(error) => status.set(format!(
                            "Could not start 3D renderer: {}",
                            error.as_string().unwrap_or_else(|| "unknown error".into())
                        )),
                    }
                }
                move || {
                    runtime.borrow_mut().take();
                }
            },
        );
    }
    html! { <div class="modelv">
        <canvas ref={canvas} class="modelv-canvas" aria-label="Interactive 3D board model" />
        if !status.is_empty() { <div class="modelv-status">{status.as_str()}</div> }
    </div> }
}
