//! The discs: two halves turned on a lathe, white on one face and black on the other, meeting in a fine
//! groove round the edge. The disc's middle is the origin; the white face looks up the y axis.

use crate::math::{PI, Vec3, vec3};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub occlusion: f32, // light reaching into the groove
}

#[derive(Clone, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

// A square of the board is 1 wide.
pub const RADIUS: f32 = 0.42;
pub const HALF_HEIGHT: f32 = 0.06;

pub struct DiscShape {
    // The halves, for the scene.
    pub white: Mesh,
    pub black: Mesh,
    // For the reflections.
    pub simple_white: Mesh,
    pub simple_black: Mesh,
    // The whole disc, for the shadows.
    pub simple: Mesh,
}

struct ProfilePoint {
    r: f32,
    y: f32,
    occlusion: f32,
}

const SEAM_RADIUS: f32 = 0.012; // the rounded edge at the seam, where the halves meet
const EDGE_RADIUS: f32 = 0.022; // the rounded edge of the face
const FACE_EDGE: f32 = RADIUS - EDGE_RADIUS;
const RING: f32 = 0.335; // a fine groove round the face

/// A smooth dip of width 2 * half_width: 1 in the middle, 0 at the edges.
fn bump(x: f32, center: f32, half_width: f32) -> f32 {
    let t = (x - center) / half_width;
    if t.abs() < 1.0 { (1.0 - t * t) * (1.0 - t * t) } else { 0.0 }
}

/// The face: very slightly domed, with the groove near its edge.
fn face_height(r: f32) -> f32 {
    HALF_HEIGHT + 0.003 * (1.0 - (r / FACE_EDGE) * (r / FACE_EDGE)) - 0.0022 * bump(r, RING, 0.006)
}

fn face_occlusion(r: f32) -> f32 {
    1.0 - 0.35 * bump(r, RING, 0.008)
}

/// The outline of the white half, from the seam round the edge and over the face to its middle. The face is
/// nearly flat: its points crowd round the groove and thin out towards the middle.
fn profile(detailed: bool) -> Vec<ProfilePoint> {
    let mut points = Vec::new();

    // The rounded edge at the seam, darker deep in the groove.
    let seam = if detailed { 5 } else { 2 };
    for i in 0..seam {
        let t = i as f32 / seam as f32;
        let angle = -PI / 2.0 + PI / 2.0 * t;
        points.push(ProfilePoint {
            r: RADIUS - SEAM_RADIUS + SEAM_RADIUS * angle.cos(),
            y: SEAM_RADIUS + SEAM_RADIUS * angle.sin(),
            occlusion: 0.45 + 0.55 * t,
        });
    }
    // The straight side.
    points.push(ProfilePoint { r: RADIUS, y: SEAM_RADIUS, occlusion: 1.0 });

    // The edge of the face.
    let edge = if detailed { 8 } else { 3 };
    for i in 0..edge {
        let angle = PI / 2.0 * i as f32 / edge as f32;
        points.push(ProfilePoint { r: FACE_EDGE + EDGE_RADIUS * angle.cos(), y: HALF_HEIGHT - EDGE_RADIUS + EDGE_RADIUS * angle.sin(), occlusion: 1.0 });
    }

    // The face, inwards.
    let radii: Vec<f32> = if detailed {
        let outer = [FACE_EDGE, 0.385, 0.37, 0.355];
        let groove = (-4..=4).map(|i| RING - 0.003 * i as f32);
        let inner = [0.31, 0.28, 0.24, 0.19, 0.13, 0.07, 0.0];
        outer.into_iter().chain(groove).chain(inner).collect()
    } else {
        vec![FACE_EDGE, 0.25, 0.0]
    };
    for r in radii {
        points.push(ProfilePoint { r, y: face_height(r), occlusion: if detailed { face_occlusion(r) } else { 1.0 } });
    }
    points
}

/// The profile turned round the y axis; `mirrored` turns it upside down, for the black half. Normals come from
/// the neighbouring points of the outline.
fn lathe(profile: &[ProfilePoint], segments: u32, mirrored: bool) -> Mesh {
    let count = profile.len();
    let mut normals: Vec<(f32, f32)> = (0..count)
        .map(|i| {
            let (before, after) = (&profile[i.saturating_sub(1)], &profile[(i + 1).min(count - 1)]);
            let (dr, dy) = (after.r - before.r, after.y - before.y);
            let length = dr.hypot(dy);
            if length > 0.0 { (dy / length, -dr / length) } else { (0.0, 1.0) }
        })
        .collect();
    normals[0] = (0.0, -1.0); // the seam faces the other half
    normals[count - 1] = (0.0, 1.0); // the middle of the face
    let sign = if mirrored { -1.0 } else { 1.0 };

    let mut mesh = Mesh::default();
    for (point, normal) in profile.iter().zip(&normals) {
        for j in 0..=segments {
            let (s, c) = (2.0 * PI * j as f32 / segments as f32).sin_cos();
            mesh.vertices.push(Vertex {
                position: vec3(point.r * c, sign * point.y, point.r * s),
                normal: vec3(normal.0 * c, sign * normal.1, normal.0 * s).normalized(),
                occlusion: point.occlusion,
            });
        }
    }

    let index = |i: usize, j: u32| i as u32 * (segments + 1) + j;
    // Mirroring turns the winding round; it is turned back so that the outside stays the front.
    let mut triangle = |a: u32, b: u32, c: u32| mesh.indices.extend(if mirrored { [a, c, b] } else { [a, b, c] });
    for i in 0..count - 1 {
        for j in 0..segments {
            // Next to the axis a quad narrows to a triangle.
            if profile[i].r > 0.0 {
                triangle(index(i, j), index(i + 1, j + 1), index(i, j + 1));
            }
            if profile[i + 1].r > 0.0 {
                triangle(index(i, j), index(i + 1, j), index(i + 1, j + 1));
            }
        }
    }
    mesh
}

fn merge(a: &Mesh, b: &Mesh) -> Mesh {
    let mut mesh = a.clone();
    let offset = a.vertices.len() as u32;
    mesh.vertices.extend(&b.vertices);
    mesh.indices.extend(b.indices.iter().map(|index| index + offset));
    mesh
}

pub fn build_disc() -> DiscShape {
    let (detailed, simple) = (profile(true), profile(false));
    let (simple_white, simple_black) = (lathe(&simple, 32, false), lathe(&simple, 32, true));
    DiscShape { white: lathe(&detailed, 96, false), black: lathe(&detailed, 96, true), simple: merge(&simple_white, &simple_black), simple_white, simple_black }
}
