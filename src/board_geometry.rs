//! The board, its frame and the table. A square is one unit; the playing surface is the plane y = 0. Row 1
//! is at the far side seen from the default view, from +z, as at the top of a printed board: a1 is the far left corner.

use crate::math::{PI, Vec2, Vec3, vec2, vec3};
use crate::reversi::{self, Square};

pub const FRAME_INNER: f32 = 4.0;
pub const FRAME_OUTER: f32 = 4.62;
pub const FRAME_TOP: f32 = 0.03;
pub const TABLE_HEIGHT: f32 = -0.32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
}

#[derive(Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

pub fn square_center(square: Square) -> Vec3 {
    vec3(reversi::file_of(square) as f32 - 3.5, 0.0, reversi::row_of(square) as f32 - 3.5)
}

/// The square under a point of the playing surface, or -1.
pub fn square_at(point: Vec3) -> Square {
    reversi::square_at((point.x + 4.0).floor() as i32, (point.z + 4.0).floor() as i32)
}

/// Adds a quad, wound counterclockwise as seen from the side its normal faces.
fn add_quad(mesh: &mut Mesh, corners: [Vertex; 4]) {
    let base = mesh.vertices.len() as u32;
    let [a, b, c, d] = corners;
    let facing = (b.position - a.position).cross(c.position - a.position);
    let normal = a.normal + b.normal + c.normal + d.normal;
    mesh.vertices.extend(corners);
    if facing.dot(normal) >= 0.0 {
        mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        mesh.indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

pub fn build_squares() -> Mesh {
    let mut mesh = Mesh::default();
    let vertex = |x, z, u, v| Vertex { position: vec3(x, 0.0, z), normal: Vec3::Y, uv: vec2(u, v) };
    add_quad(&mut mesh, [vertex(-4.0, 4.0, 0.0, 0.0), vertex(4.0, 4.0, 1.0, 0.0), vertex(4.0, -4.0, 1.0, 1.0), vertex(-4.0, -4.0, 0.0, 1.0)]);
    mesh
}

/// The frame's cross-section, swept round the board with mitred corners: a rounded inner edge rising from
/// the squares, a flat top with an eased outer edge, and the side down to the table. The inner edge is a
/// quarter circle rather than a step, so that it does not shrink to a flickering sliver when seen from afar.
pub fn build_frame() -> Mesh {
    struct ProfilePoint {
        // Outward from the playing area's edge, and height.
        out: f32,
        y: f32,
        // The normal.
        n_out: f32,
        n_y: f32,
    }

    const fn point(out: f32, y: f32, n_out: f32, n_y: f32) -> ProfilePoint {
        ProfilePoint { out, y, n_out, n_y }
    }

    const PROFILE: [ProfilePoint; 11] = [
        point(0.000, -0.002, -1.0, 0.0),
        point(0.000, 0.000, -1.0, 0.0),
        point(0.0023, 0.0115, -0.924, 0.383),
        point(0.0088, 0.0212, -0.707, 0.707),
        point(0.0185, 0.0277, -0.383, 0.924),
        point(0.030, FRAME_TOP, 0.0, 1.0),
        point(0.585, FRAME_TOP, 0.0, 1.0),
        point(0.607, 0.022, 0.62, 0.78),
        point(0.618, 0.006, 0.96, 0.28),
        point(0.62, -0.012, 1.0, 0.0),
        point(0.62, TABLE_HEIGHT, 1.0, 0.0),
    ];

    const OUTWARD: [Vec3; 4] = [vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, -1.0), vec3(-1.0, 0.0, 0.0)];

    let mut mesh = Mesh::default();
    for o in OUTWARD {
        let along = vec3(-o.z, 0.0, o.x); // along the side
        for pair in PROFILE.windows(2) {
            let (p, q) = (&pair[0], &pair[1]);
            if p.out == q.out && p.y == q.y {
                continue;
            }
            let (p_extent, q_extent) = (FRAME_INNER + p.out, FRAME_INNER + q.out);
            let vertex = |point: &ProfilePoint, extent: f32, s: f32| Vertex {
                position: o * extent + along * s + vec3(0.0, point.y, 0.0),
                normal: (o * point.n_out + vec3(0.0, point.n_y, 0.0)).normalized(),
                uv: vec2(s, point.out),
            };
            add_quad(&mut mesh, [vertex(p, p_extent, -p_extent), vertex(p, p_extent, p_extent), vertex(q, q_extent, q_extent), vertex(q, q_extent, -q_extent)]);
        }
    }
    mesh
}

pub fn build_table(radius: f32) -> Mesh {
    const SEGMENTS: u32 = 96;
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex { position: vec3(0.0, TABLE_HEIGHT, 0.0), normal: Vec3::Y, uv: vec2(0.5, 0.5) });
    for i in 0..SEGMENTS {
        let (s, c) = (2.0 * PI * i as f32 / SEGMENTS as f32).sin_cos();
        mesh.vertices.push(Vertex { position: vec3(c * radius, TABLE_HEIGHT, s * radius), normal: Vec3::Y, uv: vec2(c * 0.5 + 0.5, s * 0.5 + 0.5) });
    }
    for i in 0..SEGMENTS {
        // Counterclockwise from above: from +x toward -z.
        let (a, b) = (1 + i, 1 + (i + 1) % SEGMENTS);
        mesh.indices.extend([0, b, a]);
    }
    mesh
}
