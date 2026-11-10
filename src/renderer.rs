//! Draws the scene into the window: shadows, reflections, the lit scene in high dynamic range with
//! multisampling, then bloom and tone mapping. The interface is drawn on top afterwards.

use std::mem::{offset_of, size_of};

use crate::Result;
use crate::board_geometry::{self as board, FRAME_INNER, FRAME_OUTER};
use crate::gfx::{self, Attribute, Buffer, Framebuffer, Program, Sampler, Stage, Texture, VertexArray};
use crate::math::{Mat4, Vec3, Vec4, vec2, vec3, vec4};
use crate::piece_models::{self as pieces, DiscShape};
use crate::shaders;
use crate::text::{self, FontLibrary};
use crate::themes::{PieceLook, Theme};

/// A disc: its two halves, each in the look of its color.
#[derive(Clone, Copy)]
pub struct DiscDraw {
    pub model: Mat4,
    pub ground: f32, // the height of what it stands on
    pub glow_color: Vec3,
    pub glow: f32,
    pub dissolve: f32,
    pub seed: f32, // where the grain falls
    pub mark: f32, // the disc placed last
    pub opacity: f32,
}

/// A soft dark patch round the foot of a disc.
#[derive(Clone, Copy)]
pub struct ContactShadow {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    pub strength: f32,
}

/// Squares marked on the board, as bits of file + 8 * row.
#[derive(Clone, Copy, Default)]
pub struct BoardMarks {
    pub legal: u64, // where the side to move can play
    pub hover: u64, // the legal square under the pointer
    pub last_move: u64,
    pub hint: u64,
}

/// A rectangle of the screen behind which the scene is blurred, in pixels from the top left.
#[derive(Clone, Copy)]
pub struct FrostedPanel {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub opacity: f32,
}

pub struct Scene {
    pub view: Mat4,
    pub projection: Mat4,
    pub eye: Vec3,
    pub time: f32,
    pub discs: Vec<DiscDraw>,
    pub ghost: Option<DiscDraw>, // the disc that a click would place, seen through
    pub contacts: Vec<ContactShadow>,
    pub marks: BoardMarks,
    pub panels: Vec<FrostedPanel>,
}

const SHADOW_SIZE: i32 = 2048;
const SHADOW_RADIUS: f32 = 7.6;
const SHADOW_NEAR: f32 = 4.0;
const SHADOW_FAR: f32 = 36.0;
const ENVIRONMENT_SIZE: i32 = 512;
const IRRADIANCE_SIZE: i32 = 32;
const PREFILTERED_SIZE: i32 = 256;
const PREFILTERED_LEVELS: i32 = 6;
const BRDF_SIZE: i32 = 128;
const LABEL_SIZE: i32 = 2048;
const MAX_CONTACTS: usize = 72; // as many as uDiscs in surface.frag holds
const MAX_PANELS: usize = 8; // the lit panels of the studio, in environment.comp
const MAX_FROSTED: usize = 4; // the panels of the interface, in composite.frag
const BLOOM_LEVELS: usize = 6;

/// The std140 layout of the per-frame uniform block in common.glsl.
#[repr(C)]
#[derive(Clone, Copy)]
struct FrameBlock {
    view_projection: Mat4,
    inverse_view_projection: Mat4,
    light_view_projection: Mat4,
    camera_position: Vec4,
    light_direction: Vec4,
    light_color: Vec4,
    shadow_params: Vec4,
    viewport: Vec4,
    environment: Vec4,
    accent: Vec4,
    accent_warm: Vec4,
    fog: Vec4,
}

/// The std140 layout of the per-pass uniform block.
#[repr(C)]
#[derive(Clone, Copy)]
struct PassBlock {
    eye: Vec4,
    mirror: Vec4,
}

fn levels_for(size: i32) -> i32 {
    (size.max(1).ilog2() + 1) as i32
}

struct GpuMesh {
    vertex_array: VertexArray,
    _vertices: Buffer,
    _indices: Buffer,
    count: i32,
}

impl GpuMesh {
    fn new<V: Copy>(vertices: &[V], indices: &[u32], attributes: &[Attribute]) -> GpuMesh {
        let vertex_buffer = Buffer::with_data(gfx::bytes_of(vertices));
        let index_buffer = Buffer::with_data(gfx::bytes_of(indices));
        let vertex_array = VertexArray::new(&vertex_buffer, size_of::<V>(), attributes, Some(&index_buffer));
        GpuMesh { vertex_array, _vertices: vertex_buffer, _indices: index_buffer, count: indices.len() as i32 }
    }

    fn of_disc(mesh: &pieces::Mesh) -> GpuMesh {
        let attributes = [
            Attribute { size: 3, kind: gl::FLOAT, offset: offset_of!(pieces::Vertex, position) },
            Attribute { size: 3, kind: gl::FLOAT, offset: offset_of!(pieces::Vertex, normal) },
            Attribute { size: 1, kind: gl::FLOAT, offset: offset_of!(pieces::Vertex, occlusion) },
        ];
        GpuMesh::new(&mesh.vertices, &mesh.indices, &attributes)
    }

    fn of_board(mesh: &board::Mesh) -> GpuMesh {
        let attributes = [
            Attribute { size: 3, kind: gl::FLOAT, offset: offset_of!(board::Vertex, position) },
            Attribute { size: 3, kind: gl::FLOAT, offset: offset_of!(board::Vertex, normal) },
            Attribute { size: 2, kind: gl::FLOAT, offset: offset_of!(board::Vertex, uv) },
        ];
        GpuMesh::new(&mesh.vertices, &mesh.indices, &attributes)
    }

    fn draw(&self) {
        self.vertex_array.bind();
        gfx::draw_indexed_triangles(self.count);
    }
}

/// A level of the bloom: the image at half the size of the level before.
#[derive(Default)]
struct BloomLevel {
    texture: Texture,
    framebuffer: Framebuffer,
    width: i32,
    height: i32,
}

pub struct Renderer {
    width: i32,
    height: i32,
    samples: i32,
    theme: Theme,
    light_view_projection: Mat4,

    white: GpuMesh,
    black: GpuMesh,
    simple_white: GpuMesh,
    simple_black: GpuMesh,
    simple_disc: GpuMesh,
    squares: GpuMesh,
    frame: GpuMesh,
    table: GpuMesh,
    empty_vertex_array: VertexArray,

    disc_program: Program,
    surface_program: Program,
    shadow_program: Program,
    background_program: Program,
    resolve_program: Program,
    bloom_down_program: Program,
    bloom_up_program: Program,
    composite_program: Program,
    environment_program: Program,
    irradiance_program: Program,
    prefilter_program: Program,

    frame_data: Buffer,
    pass_data: Buffer,

    shadow_map: Texture,
    shadow_framebuffer: Framebuffer,
    shadow_compare_sampler: Sampler,
    shadow_depth_sampler: Sampler,
    environment: Texture,
    irradiance: Texture,
    prefiltered: Texture,
    brdf: Texture,
    labels: Texture,

    scene_color: Texture,
    scene_depth: Texture,
    resolved: Texture,
    scene_framebuffer: Framebuffer,
    resolved_framebuffer: Framebuffer,
    reflection_color: Texture,
    reflection_depth: Texture,
    reflection_framebuffer: Framebuffer,
    bloom: [BloomLevel; BLOOM_LEVELS],
}

impl Renderer {
    pub fn new(disc: &DiscShape, fonts: &mut FontLibrary, theme: &Theme) -> Result<Renderer> {
        let scene = |name, vertex, fragment| {
            Program::new(
                name,
                &[
                    Stage { kind: gl::VERTEX_SHADER, sources: &[shaders::COMMON, vertex] },
                    Stage { kind: gl::FRAGMENT_SHADER, sources: &[shaders::COMMON, shaders::LIGHTING, fragment] },
                ],
            )
        };
        let screen = |name, fragment| {
            Program::new(
                name,
                &[
                    Stage { kind: gl::VERTEX_SHADER, sources: &[shaders::FULLSCREEN_VERT] },
                    Stage { kind: gl::FRAGMENT_SHADER, sources: &[shaders::COMMON, fragment] },
                ],
            )
        };
        let compute = |name, source| Program::new(name, &[Stage { kind: gl::COMPUTE_SHADER, sources: &[shaders::IBL_COMMON, source] }]);

        let disc_program = scene("disc", shaders::DISC_VERT, shaders::DISC_FRAG)?;
        let surface_program = scene("surface", shaders::SURFACE_VERT, shaders::SURFACE_FRAG)?;
        let shadow_program = Program::new(
            "shadow",
            &[
                Stage { kind: gl::VERTEX_SHADER, sources: &[shaders::COMMON, shaders::SHADOW_VERT] },
                Stage { kind: gl::FRAGMENT_SHADER, sources: &[shaders::COMMON, shaders::SHADOW_FRAG] },
            ],
        )?;
        let background_program = screen("background", shaders::BACKGROUND_FRAG)?;
        let resolve_program = screen("resolve", shaders::RESOLVE_FRAG)?;
        let bloom_down_program = screen("bloom", shaders::BLOOM_DOWN_FRAG)?;
        let bloom_up_program = screen("bloom", shaders::BLOOM_UP_FRAG)?;
        let composite_program = screen("composite", shaders::COMPOSITE_FRAG)?;
        let environment_program = compute("environment", shaders::ENVIRONMENT_COMP)?;
        let irradiance_program = compute("irradiance", shaders::IRRADIANCE_COMP)?;
        let prefilter_program = compute("prefilter", shaders::PREFILTER_COMP)?;
        let brdf_program = compute("brdf", shaders::BRDF_COMP)?;

        let frame_data = Buffer::with_size(size_of::<FrameBlock>());
        let pass_data = Buffer::with_size(size_of::<PassBlock>());
        frame_data.bind_uniform_block(0);
        pass_data.bind_uniform_block(1);

        // The shadow map, read both with comparison (for filtering) and without (to find the casters).
        let shadow_map = Texture::new(gl::TEXTURE_2D);
        shadow_map.storage(1, gl::DEPTH_COMPONENT32F, SHADOW_SIZE, SHADOW_SIZE);
        let shadow_framebuffer = Framebuffer::new();
        shadow_framebuffer.attach(gl::DEPTH_ATTACHMENT, &shadow_map);
        shadow_framebuffer.without_color();
        shadow_framebuffer.check("shadow")?;
        let (shadow_compare_sampler, shadow_depth_sampler) = (Sampler::new(), Sampler::new());
        for sampler in [&shadow_compare_sampler, &shadow_depth_sampler] {
            sampler.parameter(gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
            sampler.parameter(gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
        }
        shadow_compare_sampler.parameter(gl::TEXTURE_MIN_FILTER, gl::LINEAR);
        shadow_compare_sampler.parameter(gl::TEXTURE_MAG_FILTER, gl::LINEAR);
        shadow_compare_sampler.parameter(gl::TEXTURE_COMPARE_MODE, gl::COMPARE_REF_TO_TEXTURE);
        shadow_compare_sampler.parameter(gl::TEXTURE_COMPARE_FUNC, gl::LEQUAL);
        shadow_depth_sampler.parameter(gl::TEXTURE_MIN_FILTER, gl::NEAREST);
        shadow_depth_sampler.parameter(gl::TEXTURE_MAG_FILTER, gl::NEAREST);

        // The light probes, baked when the theme is set, and the table of the reflectance, baked here once.
        gfx::enable(gl::TEXTURE_CUBE_MAP_SEAMLESS);
        let cube = |size, levels| {
            let texture = Texture::new(gl::TEXTURE_CUBE_MAP);
            texture.storage(levels, gl::RGBA16F, size, size);
            texture.set_filter(if levels > 1 { gl::LINEAR_MIPMAP_LINEAR } else { gl::LINEAR }, gl::LINEAR, gl::CLAMP_TO_EDGE);
            texture
        };
        let environment = cube(ENVIRONMENT_SIZE, levels_for(ENVIRONMENT_SIZE));
        let irradiance = cube(IRRADIANCE_SIZE, 1);
        let prefiltered = cube(PREFILTERED_SIZE, PREFILTERED_LEVELS);
        let brdf = Texture::new(gl::TEXTURE_2D);
        brdf.storage(1, gl::RG16F, BRDF_SIZE, BRDF_SIZE);
        brdf.set_filter(gl::LINEAR, gl::LINEAR, gl::CLAMP_TO_EDGE);
        brdf_program.bind();
        brdf.bind_image(0, 0, false, gl::RG16F);
        gfx::dispatch_compute(BRDF_SIZE, BRDF_SIZE, 1);

        // The coordinates inlaid in the frame.
        let labels = Texture::new(gl::TEXTURE_2D);
        labels.storage(levels_for(LABEL_SIZE), gl::R8, LABEL_SIZE, LABEL_SIZE);
        labels.upload_r8(LABEL_SIZE, LABEL_SIZE, &text::render_board_labels(fonts, LABEL_SIZE, FRAME_OUTER, FRAME_INNER));
        labels.generate_mipmap();
        labels.set_filter(gl::LINEAR_MIPMAP_LINEAR, gl::LINEAR, gl::CLAMP_TO_EDGE);
        labels.parameter_f32(gfx::TEXTURE_MAX_ANISOTROPY, gfx::max_anisotropy().min(8.0));

        let samples =
            [gl::MAX_SAMPLES, gl::MAX_COLOR_TEXTURE_SAMPLES, gl::MAX_DEPTH_TEXTURE_SAMPLES].into_iter().map(gfx::get_integer).fold(4, i32::min).max(1);

        let mut renderer = Renderer {
            width: 0,
            height: 0,
            samples,
            theme: theme.clone(),
            light_view_projection: Mat4::IDENTITY,
            white: GpuMesh::of_disc(&disc.white),
            black: GpuMesh::of_disc(&disc.black),
            simple_white: GpuMesh::of_disc(&disc.simple_white),
            simple_black: GpuMesh::of_disc(&disc.simple_black),
            simple_disc: GpuMesh::of_disc(&disc.simple),
            squares: GpuMesh::of_board(&board::build_squares()),
            frame: GpuMesh::of_board(&board::build_frame()),
            table: GpuMesh::of_board(&board::build_table(40.0)),
            empty_vertex_array: VertexArray::empty(),
            disc_program,
            surface_program,
            shadow_program,
            background_program,
            resolve_program,
            bloom_down_program,
            bloom_up_program,
            composite_program,
            environment_program,
            irradiance_program,
            prefilter_program,
            frame_data,
            pass_data,
            shadow_map,
            shadow_framebuffer,
            shadow_compare_sampler,
            shadow_depth_sampler,
            environment,
            irradiance,
            prefiltered,
            brdf,
            labels,
            scene_color: Texture::default(),
            scene_depth: Texture::default(),
            resolved: Texture::default(),
            scene_framebuffer: Framebuffer::default(),
            resolved_framebuffer: Framebuffer::default(),
            reflection_color: Texture::default(),
            reflection_depth: Texture::default(),
            reflection_framebuffer: Framebuffer::default(),
            bloom: Default::default(),
        };
        renderer.set_theme(theme);
        Ok(renderer)
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.theme = theme.clone();

        let to_light = theme.key_direction.normalized();
        let up = if to_light.y.abs() > 0.99 { vec3(0.0, 0.0, 1.0) } else { Vec3::Y };
        let center = vec3(0.5, 0.0, 0.0);
        let view = Mat4::look_at(center + to_light * 20.0, center, up);
        let projection = Mat4::orthographic(-SHADOW_RADIUS, SHADOW_RADIUS, -SHADOW_RADIUS, SHADOW_RADIUS, SHADOW_NEAR, SHADOW_FAR);
        self.light_view_projection = projection * view;

        self.bake_environment();
    }

    /// The studio light of the theme, baked into the light probes.
    fn bake_environment(&self) {
        let theme = &self.theme;
        let environment = &self.environment_program;
        environment.set("uCeiling", theme.ceiling);
        environment.set("uHorizon", theme.horizon);
        environment.set("uFloor", theme.floor);
        let (mut directions, mut shapes, mut colors) = ([Vec4::default(); MAX_PANELS], [Vec4::default(); MAX_PANELS], [Vec4::default(); MAX_PANELS]);
        let count = theme.panels.len().min(MAX_PANELS);
        for (i, panel) in theme.panels.iter().take(count).enumerate() {
            directions[i] = panel.direction.extend(panel.roll);
            shapes[i] = vec4(panel.half_size.x, panel.half_size.y, panel.corner, panel.softness);
            colors[i] = panel.radiance.extend(panel.falloff);
        }
        environment.set("uPanelDirection", &directions[..]);
        environment.set("uPanelShape", &shapes[..]);
        environment.set("uPanelColor", &colors[..]);
        environment.set("uPanelCount", count as i32);
        environment.bind();
        self.environment.bind_image(0, 0, true, gl::RGBA16F);
        gfx::dispatch_compute(ENVIRONMENT_SIZE, ENVIRONMENT_SIZE, 6);
        self.environment.generate_mipmap();

        self.environment.bind(0);
        self.irradiance_program.set("uSourceLod", 4.0f32);
        self.irradiance_program.bind();
        self.irradiance.bind_image(0, 0, true, gl::RGBA16F);
        gfx::dispatch_compute(IRRADIANCE_SIZE, IRRADIANCE_SIZE, 6);

        self.prefilter_program.set("uSourceSize", ENVIRONMENT_SIZE as f32);
        self.prefilter_program.bind();
        for level in 0..PREFILTERED_LEVELS {
            let size = PREFILTERED_SIZE >> level;
            self.prefilter_program.set("uRoughness", level as f32 / (PREFILTERED_LEVELS - 1) as f32);
            self.prefiltered.bind_image(0, level, true, gl::RGBA16F);
            gfx::dispatch_compute(size, size, 6);
        }
    }

    pub fn resize(&mut self, width: i32, height: i32) -> Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        if width == self.width && height == self.height {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self.create_screen_targets()
    }

    fn create_screen_targets(&mut self) -> Result<()> {
        let (width, height) = (self.width, self.height);
        self.scene_color = Texture::new(gl::TEXTURE_2D_MULTISAMPLE);
        self.scene_color.storage_multisample(self.samples, gl::RGBA16F, width, height);
        self.scene_depth = Texture::new(gl::TEXTURE_2D_MULTISAMPLE);
        self.scene_depth.storage_multisample(self.samples, gl::DEPTH_COMPONENT32F, width, height);
        self.scene_framebuffer = Framebuffer::new();
        self.scene_framebuffer.attach(gl::COLOR_ATTACHMENT0, &self.scene_color);
        self.scene_framebuffer.attach(gl::DEPTH_ATTACHMENT, &self.scene_depth);
        self.scene_framebuffer.check("scene")?;

        self.resolved = Texture::new(gl::TEXTURE_2D);
        self.resolved.storage(1, gl::RGBA16F, width, height);
        self.resolved.set_filter(gl::LINEAR, gl::LINEAR, gl::CLAMP_TO_EDGE);
        self.resolved_framebuffer = Framebuffer::new();
        self.resolved_framebuffer.attach(gl::COLOR_ATTACHMENT0, &self.resolved);
        self.resolved_framebuffer.check("resolve")?;

        // The reflections at half size, blurred through their mip levels.
        let (reflection_width, reflection_height) = ((width / 2).max(1), (height / 2).max(1));
        let reflection_levels = levels_for(reflection_width.max(reflection_height)).min(5);
        self.reflection_color = Texture::new(gl::TEXTURE_2D);
        self.reflection_color.storage(reflection_levels, gl::RGBA16F, reflection_width, reflection_height);
        self.reflection_color.set_filter(gl::LINEAR_MIPMAP_LINEAR, gl::LINEAR, gl::CLAMP_TO_EDGE);
        self.reflection_depth = Texture::new(gl::TEXTURE_2D);
        self.reflection_depth.storage(1, gl::DEPTH_COMPONENT24, reflection_width, reflection_height);
        self.reflection_framebuffer = Framebuffer::new();
        self.reflection_framebuffer.attach(gl::COLOR_ATTACHMENT0, &self.reflection_color);
        self.reflection_framebuffer.attach(gl::DEPTH_ATTACHMENT, &self.reflection_depth);
        self.reflection_framebuffer.check("reflection")?;

        for (level, bloom) in self.bloom.iter_mut().enumerate() {
            bloom.width = (width >> (level + 1)).max(1);
            bloom.height = (height >> (level + 1)).max(1);
            bloom.texture = Texture::new(gl::TEXTURE_2D);
            bloom.texture.storage(1, gl::R11F_G11F_B10F, bloom.width, bloom.height);
            bloom.texture.set_filter(gl::LINEAR, gl::LINEAR, gl::CLAMP_TO_EDGE);
            bloom.framebuffer = Framebuffer::new();
            bloom.framebuffer.attach(gl::COLOR_ATTACHMENT0, &bloom.texture);
            bloom.framebuffer.check("bloom")?;
        }
        Ok(())
    }

    pub fn render(&self, scene: &Scene) {
        self.update_frame_data(scene);
        self.draw_shadow_map(scene);
        self.draw_reflections(scene);
        self.draw_main_pass(scene);
        self.post_process(scene);
    }

    fn update_frame_data(&self, scene: &Scene) {
        let theme = &self.theme;
        let view_projection = scene.projection * scene.view;
        let (width, height) = (self.width as f32, self.height as f32);
        let block = FrameBlock {
            view_projection,
            inverse_view_projection: view_projection.inverse(),
            light_view_projection: self.light_view_projection,
            camera_position: scene.eye.extend(scene.time),
            light_direction: theme.key_direction.normalized().extend(2.0 * SHADOW_RADIUS / SHADOW_SIZE as f32),
            light_color: theme.key_radiance.extend(theme.key_softness),
            shadow_params: vec4(SHADOW_FAR - SHADOW_NEAR, 0.00025, 0.0, 0.0),
            viewport: vec4(width, height, 1.0 / width, 1.0 / height),
            environment: vec4(theme.diffuse_light, theme.reflected_light, (PREFILTERED_LEVELS - 1) as f32, theme.background),
            accent: theme.accent.extend(1.0),
            accent_warm: theme.accent_warm.extend(1.0),
            fog: theme.fog.extend(0.05),
        };
        self.frame_data.write(gfx::bytes_of(&[block]));
    }

    fn set_pass(&self, eye: Vec3, shadows: bool, mirror: bool) {
        let block = PassBlock { eye: eye.extend(if shadows { 0.0 } else { 1.0 }), mirror: vec4(0.0, if mirror { 1.0 } else { 0.0 }, 0.0, 0.0) };
        self.pass_data.write(gfx::bytes_of(&[block]));
    }

    fn bind_lighting(&self, reflections: bool) {
        self.shadow_map.bind(0);
        self.shadow_compare_sampler.bind(0);
        self.shadow_map.bind(1);
        self.shadow_depth_sampler.bind(1);
        self.irradiance.bind(2);
        self.prefiltered.bind(3);
        self.brdf.bind(4);
        if reflections {
            self.reflection_color.bind(5);
        } else {
            gfx::unbind_texture(5);
        }
        self.labels.bind(6);
    }

    fn draw_shadow_map(&self, scene: &Scene) {
        self.shadow_framebuffer.bind();
        gfx::viewport(SHADOW_SIZE, SHADOW_SIZE);
        self.shadow_framebuffer.clear_depth(1.0);
        gfx::enable(gl::DEPTH_TEST);
        gfx::depth_func(gl::LESS);
        gfx::depth_mask(true);
        gfx::disable(gl::CULL_FACE);
        gfx::enable(gl::POLYGON_OFFSET_FILL);
        gfx::polygon_offset(1.5, 3.0);

        let program = &self.shadow_program;
        program.bind();
        program.set("uModel", Mat4::IDENTITY);
        program.set("uDissolve", 0.0f32);
        self.frame.draw();
        self.squares.draw();
        for disc in &scene.discs {
            program.set("uModel", disc.model);
            program.set("uDissolve", disc.dissolve);
            program.set("uSeed", disc.seed);
            self.simple_disc.draw();
        }

        gfx::disable(gl::POLYGON_OFFSET_FILL);
    }

    fn draw_reflections(&self, scene: &Scene) {
        self.reflection_framebuffer.bind();
        gfx::viewport((self.width / 2).max(1), (self.height / 2).max(1));
        self.reflection_framebuffer.clear_color([0.0; 4]);
        self.reflection_framebuffer.clear_depth(1.0);

        // The discs turned upside down under the board, seen from the camera, as if from below the board.
        self.set_pass(vec3(scene.eye.x, -scene.eye.y, scene.eye.z), false, true);
        self.bind_lighting(false);
        gfx::enable(gl::CLIP_DISTANCE0);
        gfx::enable(gl::DEPTH_TEST);
        gfx::depth_func(gl::LESS);
        gfx::enable(gl::CULL_FACE);
        gfx::cull_face(gl::BACK);
        gfx::front_face(gl::CW);
        self.draw_discs(scene, false);
        gfx::front_face(gl::CCW);
        gfx::disable(gl::CLIP_DISTANCE0);
        self.reflection_color.generate_mipmap();
    }

    fn set_disc_material(&self, disc: &DiscDraw, look: &PieceLook) {
        let program = &self.disc_program;
        let accent = self.theme.accent;
        program.set("uModel", disc.model);
        program.set("uAlbedo", look.albedo);
        program.set("uSurface", vec4(look.roughness, look.metallic, look.clearcoat, look.clearcoat_roughness));
        program.set("uStyle", vec4(look.subsurface, look.pattern_strength, look.pattern, disc.ground));
        program.set("uGlow", disc.glow_color.extend(disc.glow));
        program.set("uTrim", look.trim.extend(look.trim_strength));
        program.set("uDissolve", vec4(disc.dissolve, accent.x, accent.y, accent.z));
        program.set("uDetail", vec4(disc.seed, disc.mark, disc.opacity, 0.0));
    }

    fn draw_disc(&self, disc: &DiscDraw, detailed: bool) {
        self.set_disc_material(disc, &self.theme.white);
        if detailed { &self.white } else { &self.simple_white }.draw();
        self.set_disc_material(disc, &self.theme.black);
        if detailed { &self.black } else { &self.simple_black }.draw();
    }

    fn draw_discs(&self, scene: &Scene, detailed: bool) {
        self.disc_program.bind();
        for disc in &scene.discs {
            self.draw_disc(disc, detailed);
        }
    }

    fn set_surface_material(&self, scene: &Scene) {
        let p = &self.surface_program;
        let t = &self.theme;
        p.set("uBoardColor", t.board_color);
        p.set("uBoardSurface", vec4(t.board_roughness, t.board_clearcoat, t.board_clearcoat_roughness, t.board_pattern));
        p.set("uBoardReflection", vec4(t.reflection, t.reflection_blur, t.vein_brightness, 0.0));
        p.set("uGridColor", t.grid_color);
        p.set("uGrid", vec4(t.grid_width, t.grid_metallic, t.grid_roughness, t.grid_glow));
        p.set("uStarRadius", t.star_radius);
        p.set("uFrameColor", t.frame_color);
        p.set("uFrameSurface", vec4(t.frame_roughness, t.frame_metallic, t.frame_clearcoat, t.frame_pattern));
        p.set("uInlayColor", t.inlay_color);
        p.set("uInlay", vec4(t.inlay_metallic, t.inlay_roughness, t.inlay_glow, 0.0));
        p.set("uTableColor", t.table_color);
        p.set("uTableSurface", vec4(t.table_roughness, t.table_pattern, t.table_sheen, t.table_glow));
        p.set("uLegal", scene.marks.legal);
        p.set("uHover", scene.marks.hover);
        p.set("uLastMove", scene.marks.last_move);
        p.set("uHint", scene.marks.hint);
        let discs: Vec<Vec4> = scene.contacts.iter().take(MAX_CONTACTS).map(|contact| vec4(contact.x, contact.z, contact.radius, contact.strength)).collect();
        p.set("uDiscs", discs.as_slice());
        p.set("uDiscCount", discs.len() as i32);
    }

    fn draw_main_pass(&self, scene: &Scene) {
        let fog = self.theme.fog;
        self.scene_framebuffer.bind();
        gfx::viewport(self.width, self.height);
        self.scene_framebuffer.clear_color([fog.x, fog.y, fog.z, 1.0]);
        self.scene_framebuffer.clear_depth(1.0);

        self.set_pass(scene.eye, true, false);
        self.bind_lighting(true);
        gfx::enable(gl::DEPTH_TEST);
        gfx::depth_func(gl::LESS);
        gfx::depth_mask(true);
        gfx::enable(gl::CULL_FACE);
        gfx::cull_face(gl::BACK);

        self.set_surface_material(scene);
        self.surface_program.bind();
        self.surface_program.set("uKind", 2);
        self.table.draw();
        self.surface_program.set("uKind", 1);
        self.frame.draw();
        self.surface_program.set("uKind", 0);
        self.squares.draw();

        self.draw_discs(scene, true);

        gfx::depth_func(gl::LEQUAL);
        gfx::depth_mask(false);
        self.background_program.bind();
        self.empty_vertex_array.bind();
        gfx::draw_triangles(3);

        // The disc a click would place, blended over the rest.
        if let Some(ghost) = &scene.ghost {
            gfx::enable(gl::BLEND);
            gfx::blend_func(gl::SRC_ALPHA, gl::ONE_MINUS_SRC_ALPHA);
            self.disc_program.bind();
            self.draw_disc(ghost, true);
            gfx::disable(gl::BLEND);
        }

        gfx::depth_mask(true);
        gfx::depth_func(gl::LESS);
    }

    fn post_process(&self, scene: &Scene) {
        gfx::disable(gl::DEPTH_TEST);
        gfx::disable(gl::CULL_FACE);
        self.empty_vertex_array.bind();
        self.resolved_framebuffer.bind();
        gfx::viewport(self.width, self.height);
        self.resolve_program.set("uSamples", self.samples);
        self.resolve_program.bind();
        self.scene_color.bind(0);
        gfx::draw_triangles(3);

        // Bloom: halve down to a small image, then add the levels back up.
        self.bloom_down_program.bind();
        let (mut source, mut source_width, mut source_height) = (&self.resolved, self.width, self.height);
        for (level, bloom) in self.bloom.iter().enumerate() {
            bloom.framebuffer.bind();
            gfx::viewport(bloom.width, bloom.height);
            source.bind(0);
            self.bloom_down_program.set("uSourceTexel", vec2(1.0 / source_width as f32, 1.0 / source_height as f32));
            self.bloom_down_program.set("uFirst", i32::from(level == 0));
            gfx::draw_triangles(3);
            (source, source_width, source_height) = (&bloom.texture, bloom.width, bloom.height);
        }

        self.bloom_up_program.bind();
        gfx::enable(gl::BLEND);
        gfx::blend_func(gl::ONE, gl::ONE);
        for level in (0..BLOOM_LEVELS - 1).rev() {
            let (target, smaller) = (&self.bloom[level], &self.bloom[level + 1]);
            target.framebuffer.bind();
            gfx::viewport(target.width, target.height);
            smaller.texture.bind(0);
            self.bloom_up_program.set("uSourceTexel", vec2(1.0 / smaller.width as f32, 1.0 / smaller.height as f32));
            gfx::draw_triangles(3);
        }
        gfx::disable(gl::BLEND);

        gfx::bind_window_framebuffer();
        gfx::viewport(self.width, self.height);
        let composite = &self.composite_program;
        composite.set("uExposure", self.theme.exposure);
        composite.set("uBloomStrength", self.theme.bloom);
        composite.set("uVignette", self.theme.vignette);
        let (mut panels, mut styles) = ([Vec4::default(); MAX_FROSTED], [Vec4::default(); MAX_FROSTED]);
        let count = scene.panels.len().min(MAX_FROSTED);
        for (i, panel) in scene.panels.iter().take(count).enumerate() {
            panels[i] = vec4(panel.x + panel.width / 2.0, panel.y + panel.height / 2.0, panel.width / 2.0, panel.height / 2.0);
            styles[i] = vec4(panel.radius, panel.opacity, 0.0, 0.0);
        }
        composite.set("uPanels", &panels[..]);
        composite.set("uPanelStyle", &styles[..]);
        composite.set("uPanelCount", count as i32);
        composite.bind();
        self.resolved.bind(0);
        self.bloom[0].texture.bind(1);
        self.bloom[2].texture.bind(2);
        gfx::draw_triangles(3);
    }
}
