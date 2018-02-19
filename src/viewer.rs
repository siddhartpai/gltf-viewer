use std::f32::consts::PI;
use std::fs::File;
use std::os::raw::c_void;
use std::path::Path;
use std::process;
use std::time::Instant;

use cgmath::{ Point3 };
use collision::Aabb;
use gl;
use glutin;
use glutin::{
    Api,
    MouseScrollDelta,
    MouseButton,
    GlContext,
    GlRequest,
    GlProfile,
    VirtualKeyCode,
    WindowEvent,
};
use glutin::ElementState::*;

use gltf_importer;
use gltf_importer::config::ValidationStrategy;
use image::{DynamicImage, ImageFormat};


use controls::{OrbitControls, NavState};
use controls::CameraMovement::*;
use framebuffer::Framebuffer;
use render::*;
use render::math::*;
use utils::{print_elapsed, FrameTimer, gl_check_error, print_context_info};

extern crate color_thief;
extern crate image;
use self::color_thief::{ColorFormat};

// TODO!: complete and pass through draw calls? or get rid of multiple shaders?
// How about state ordering anyway?
// struct DrawState {
//     current_shader: ShaderFlags,
//     back_face_culling_enabled: bool
// }

pub struct CameraOptions {
    pub index: i32,
    pub position: Option<Vector3>,
    pub target: Option<Vector3>,
    pub fovy: f32,
}

pub struct GltfViewer {
    width: u32,
    height: u32,

    orbit_controls: OrbitControls,
    first_mouse: bool,
    last_x: f32,
    last_y: f32,
    events_loop: Option<glutin::EventsLoop>,
    gl_window: Option<glutin::GlWindow>,

    // TODO!: get rid of scene?
    root: Root,
    scene: Scene,

    delta_time: f64, // seconds
    last_frame: Instant,

    render_timer: FrameTimer,
}

/// Note about `headless` and `visible`: True headless rendering doesn't work on
/// all operating systems, but an invisible window usually works
impl GltfViewer {
    pub fn new(source: &str, width: u32, height: u32, headless: bool, visible: bool, camera_options: CameraOptions) -> GltfViewer {
        let gl_request = GlRequest::Specific(Api::OpenGl, (3, 3));
        let gl_profile = GlProfile::Core;
        let (events_loop, gl_window, width, height) =
            if headless {
                let headless_context = glutin::HeadlessRendererBuilder::new(width, height)
                    // .with_gl(gl_request)
                    // .with_gl_profile(gl_profile)
                    .build()
                    .unwrap();
                unsafe { headless_context.make_current().unwrap() }
                gl::load_with(|symbol| headless_context.get_proc_address(symbol) as *const _);
                let framebuffer = Framebuffer::new(width, height);
                framebuffer.bind();
                unsafe { gl::Viewport(0, 0, width as i32, height as i32); }

                (None, None, width, height) // TODO: real height (retina?)
            }
            else {
                // glutin: initialize and configure
                let events_loop = glutin::EventsLoop::new();

                // TODO?: hints for 4.1, core profile, forward compat
                let window = glutin::WindowBuilder::new()
                        .with_title("gltf-viewer")
                        .with_dimensions(width, height)
                        .with_visibility(visible);

                let context = glutin::ContextBuilder::new()
                    .with_gl(gl_request)
                    .with_gl_profile(gl_profile)
                    .with_vsync(true);
                let gl_window = glutin::GlWindow::new(window, context, &events_loop).unwrap();

                // Real dimensions might be much higher on High-DPI displays
                let (real_width, real_height) = gl_window.get_inner_size().unwrap();

                unsafe { gl_window.make_current().unwrap(); }

                // gl: load all OpenGL function pointers
                gl::load_with(|symbol| gl_window.get_proc_address(symbol) as *const _);

                (Some(events_loop), Some(gl_window), real_width, real_height)
            };

        let mut orbit_controls = OrbitControls::new(
            Point3::new(0.0, 0.0, 2.0), width as f32, height as f32
        );
        orbit_controls.camera = Camera::default();
        orbit_controls.camera.fovy = camera_options.fovy;
        orbit_controls.camera.update_aspect_ratio(width as f32 / height as f32); // updates projection matrix

        let first_mouse = true;
        let last_x: f32 = width as f32 / 2.0;
        let last_y: f32 = height as f32 / 2.0;

        unsafe {
            print_context_info();

            gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);

            if headless || !visible {
                // transparent background for screenshots
                gl::ClearColor(0.0, 0.0, 0.0, 0.0);
            }
            else {
                gl::ClearColor(0.1, 0.2, 0.3, 1.0);
            }

            gl::Enable(gl::DEPTH_TEST);

            // TODO: keyboard switch?
            // draw in wireframe
            // gl::PolygonMode(gl::FRONT_AND_BACK, gl::LINE);
        };

        let (root, scene) = Self::load(source);
        let mut viewer = GltfViewer {
            width,
            height,

            orbit_controls,
            first_mouse, last_x, last_y,

            events_loop,
            gl_window,

            root,
            scene,

            delta_time: 0.0, // seconds
            last_frame: Instant::now(),

            render_timer: FrameTimer::new("rendering", 300),
        };
        unsafe { gl_check_error!(); };

        if !viewer.root.camera_nodes.is_empty() && !camera_options.index == -1 {
            if camera_options.index >= viewer.root.camera_nodes.len() as i32 {
                error!("No camera with index {} found in glTF file (max: {})",
                    camera_options.index, viewer.root.camera_nodes.len() - 1);
                process::exit(2)
            }
            let cam_node = &viewer.root.get_camera_node(camera_options.index as usize);
            viewer.orbit_controls.set_camera(
                cam_node.camera.as_ref().unwrap(),
                &cam_node.final_transform);

            if camera_options.position.is_some() || camera_options.target.is_some() {
                warn!("Ignoring --cam-pos / --cam-target since --cam-index is given.")
            }
        } else {
            viewer.set_camera_from_bounds();

            if let Some(p) = camera_options.position {
                viewer.orbit_controls.position = Point3::from_vec(p)
            }
            if let Some(target) = camera_options.target {
                viewer.orbit_controls.target = Point3::from_vec(target)
            }
        }

        viewer
    }

    pub fn load(source: &str) -> (Root, Scene) {
        let mut start_time = Instant::now();
        // TODO!: http source
        // let gltf =
        if source.starts_with("http") {
            panic!("not implemented: HTTP support temporarily removed.")
            // let http_source = HttpSource::new(source);
            // let import = gltf::Import::custom(http_source, Default::default());
            // let gltf = import_gltf(import);
            // println!(); // to end the "progress dots"
            // gltf
        }
        //     else {
        let config = gltf_importer::Config { validation_strategy: ValidationStrategy::Complete };
        let (gltf, buffers) = match gltf_importer::import_with_config(source, config) {
            Ok((gltf, buffers)) => (gltf, buffers),
            Err(err) => {
                error!("glTF import failed: {:?}", err);
                if let gltf_importer::Error::Io(_) = err {
                    error!("Hint: Are the .bin file(s) referenced by the .gltf file available?")
                }
                process::exit(1)
            },
        };

        print_elapsed("Imported glTF in ", &start_time);
        start_time = Instant::now();

        // load first scene
        if gltf.scenes().len() > 1 {
            warn!("Found more than 1 scene, can only load first at the moment.")
        }
        let base_path = Path::new(source);
        let mut root = Root::from_gltf(&gltf, &buffers, base_path);
        let scene = Scene::from_gltf(&gltf.scenes().nth(0).unwrap(), &mut root);
        print_elapsed(&format!("Loaded scene with {} nodes, {} meshes in ",
        gltf.nodes().count(), root.meshes.len()), &start_time);

        (root, scene)
    }

    /// determine "nice" camera perspective from bounding box. Inspired by donmccurdy/three-gltf-viewer
    fn set_camera_from_bounds(&mut self) {
        let bounds = &self.scene.bounds;
        let size = (bounds.max - bounds.min).magnitude();
        let center = bounds.center();

        let _max_distance = size * 10.0;
        // TODO: x,y addition optional, z optionally minus instead
        let cam_pos = Point3::new(
            center.x + size / 2.0,
            center.y + size / 5.0,
            center.z + size / 2.0,
        );
        let _near = size / 100.0;
        let _far = size * 100.0;

        self.orbit_controls.position = cam_pos;
        self.orbit_controls.target = center;

        // TODO!: set near, far, max_distance, obj_pos_modifier...
    }

    pub fn start_render_loop(&mut self) {
        loop {
            // per-frame time logic
            // NOTE: Deliberately ignoring the seconds of `elapsed()`
            self.delta_time = f64::from(self.last_frame.elapsed().subsec_nanos()) / 1_000_000_000.0;
            self.last_frame = Instant::now();

            // events
            let keep_running = process_events(
                &mut self.events_loop.as_mut().unwrap(), self.gl_window.as_mut().unwrap(),
                &mut self.orbit_controls,
                &mut self.width, &mut self.height);
            if !keep_running {
                unsafe { gl_check_error!(); } // final error check so errors don't go unnoticed
                break
            }

            self.orbit_controls.frame_update(self.delta_time); // keyboard navigation

            self.draw();

            self.gl_window.as_ref().unwrap().swap_buffers().unwrap();
        }
    }

    // Returns whether to keep running
    pub fn draw(&mut self) {
        // render
        unsafe {
            self.render_timer.start();

            gl::Clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);

            let cam_params = self.orbit_controls.camera_params();
            self.scene.draw(&mut self.root, &cam_params);

            self.render_timer.end();
        }
    }
    fn find_color_type(&mut self, t: image::ColorType) -> ColorFormat {
        match t {
            image::ColorType::RGB(8) => ColorFormat::Rgb,
            image::ColorType::RGBA(8) => ColorFormat::Rgba,
            _ => unreachable!(),
        }
    }

    pub fn screenshot(&mut self, filename: &str, width: u32, height: u32) {
        self.draw();

        let mut img = DynamicImage::new_rgba8(width, height);
        unsafe {
            let pixels = img.as_mut_rgba8().unwrap();
            gl::PixelStorei(gl::PACK_ALIGNMENT, 1);
            gl::ReadPixels(0, 0, width as i32, height as i32, gl::RGBA,
                gl::UNSIGNED_BYTE, pixels.as_mut_ptr() as *mut c_void);
            gl_check_error!();
        }

        let img = img.flipv();

        let color_type = self.find_color_type(img.color());
        let colors = color_thief::get_palette(&img.raw_pixels(), color_type, 1, 2).unwrap();
        println!("{:?}",colors);
        let mut file = File::create(filename).unwrap();
        if let Err(err) = img.save(&mut file, ImageFormat::PNG) {
            error!("{}", err);
        }
        else {
            println!("Saved {}x{} screenshot to {}", width, height, filename);
        }
    }
    pub fn multiscreenshot(&mut self, filename: &str, width: u32, height: u32, count: u32) {
        let min_angle : f32 = 0.0 ;
        let max_angle : f32 =  2.0 * PI ;
        let increment_angle : f32 = ((max_angle - min_angle)/(count as f32)) as f32;
        for i in 1..(count+1) {
            self.orbit_controls.rotate_object(increment_angle);
            let dot = filename.rfind('.').unwrap_or_else(|| filename.len());
            let mut actual_name = filename.to_string();
            actual_name.insert_str(dot, &format!("_{}", i));
            self.screenshot(&actual_name[..], width,height);
        }
    }
}

#[allow(too_many_arguments)]
fn process_events(
    events_loop: &mut glutin::EventsLoop,
    gl_window: &glutin::GlWindow,
    mut orbit_controls: &mut OrbitControls,
    width: &mut u32,
    height: &mut u32) -> bool
{
    let mut keep_running = true;
    #[allow(single_match)]
    events_loop.poll_events(|event| {
        match event {
            glutin::Event::WindowEvent{ event, .. } => match event {
                WindowEvent::Closed => keep_running = false,
                WindowEvent::Resized(w, h) => {
                    gl_window.resize(w, h);
                    *width = w;
                    *height = h;
                    let w = w as f32;
                    let h = h as f32;
                    orbit_controls.camera.update_aspect_ratio(w / h);
                    orbit_controls.screen_width = w;
                    orbit_controls.screen_height = h;

                    trace!("Resized to {}x{}", w, h);
                },
                WindowEvent::DroppedFile(_path_buf) => (), // TODO: drag file in
                WindowEvent::MouseInput { button, state: Pressed, ..} => {
                    match button {
                        MouseButton::Left => {
                            orbit_controls.state = NavState::Rotating;
                        },
                        MouseButton::Right => {
                            orbit_controls.state = NavState::Panning;
                        },
                        _ => ()
                    }
                },
                WindowEvent::MouseInput { button, state: Released, ..} => {
                    match (button, orbit_controls.state.clone()) {
                        (MouseButton::Left, NavState::Rotating) | (MouseButton::Right, NavState::Panning) => {
                            orbit_controls.state = NavState::None;
                            orbit_controls.handle_mouse_up();
                        },
                        _ => ()
                    }
                }
                WindowEvent::CursorMoved { position: (xpos, ypos), .. } => {
                    let (xpos, ypos) = (xpos as f32, ypos as f32);
                    orbit_controls.handle_mouse_move(xpos, ypos);
                },
                WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(_xoffset, yoffset), .. } => {
                    orbit_controls.process_mouse_scroll(yoffset);
                }
                WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(_rows, lines), .. } => {
                    orbit_controls.process_mouse_scroll(lines * 3.0);
                }
                WindowEvent::KeyboardInput { input, .. } => {
                    keep_running = process_input(input, &mut orbit_controls);
                }
                _ => ()
            },
            _ => ()
        }
    });

    keep_running
}

fn process_input(input: glutin::KeyboardInput, controls: &mut OrbitControls) -> bool {
    let pressed = match input.state {
        Pressed => true,
        Released => false
    };
    if let Some(code) = input.virtual_keycode {
        match code {
            VirtualKeyCode::Escape if pressed => return false,
            VirtualKeyCode::W | VirtualKeyCode::Up    => controls.process_keyboard(FORWARD, pressed),
            VirtualKeyCode::S | VirtualKeyCode::Down  => controls.process_keyboard(BACKWARD, pressed),
            VirtualKeyCode::A | VirtualKeyCode::Left  => controls.process_keyboard(LEFT, pressed),
            VirtualKeyCode::D | VirtualKeyCode::Right => controls.process_keyboard(RIGHT, pressed),
            _ => ()
        }
    }
    true
}
