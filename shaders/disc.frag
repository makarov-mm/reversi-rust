// One half of a disc: plastic, wood, stone, acrylic or metal, with a glowing ring for the neon set, a mark
// on the disc placed last, a glow when a move would turn it, and a dissolve.

in vec3 vWorld;
in vec3 vNormal;
in vec3 vLocal;
in float vOcclusion;

uniform mat4 uModel;
uniform vec3 uAlbedo;
uniform vec4 uSurface;  // roughness, metallic, clearcoat, clearcoat roughness
uniform vec4 uStyle;    // x: subsurface, y: pattern strength, z: pattern (0 none, 1 wood, 2 turned metal, 3 stone), w: height of the ground
uniform vec4 uGlow;     // rgb, strength
uniform vec4 uTrim;     // rgb, strength: a glowing ring in the groove on the face and along the seam
uniform vec4 uDissolve; // x: 0 solid to 1 gone, yzw: edge color
uniform vec4 uDetail;   // x: a seed for the grain, y: the mark of the last move, z: opacity

layout(location = 0) out vec4 fragColor;

const float DiscRadius = 0.42;
const float HalfHeight = 0.06;
const float Ring = 0.335;

void main()
{
    // Measured before anything is discarded, while the neighbouring pixels still run.
    vec3 N = normalize(vNormal);
    float spread = NormalSpread(N);
    vec3 localPixel = fwidth(vLocal);
    float radiusPixel = max(fwidth(length(vLocal.xz)), 1e-5);
    float heightPixel = max(localPixel.y, 1e-5);

    vec3 emissive = vec3(0.0);
    if (uDissolve.x > 0.0) {
        float n = Fbm(vLocal * 7.0 + 3.0 + uDetail.x * 5.0);
        float threshold = uDissolve.x * 1.2 - 0.1;
        if (n < threshold)
            discard;
        emissive += uDissolve.yzw * (1.0 - smoothstep(0.0, 0.06, n - threshold)) * 8.0;
    }

    if (!gl_FrontFacing)
        N = -N;
    vec3 V = normalize(uEye.xyz - vWorld);

    Material m = DefaultMaterial(uAlbedo, uSurface.x);
    m.metallic = uSurface.y;
    m.clearcoat = uSurface.z;
    m.clearcoatRoughness = uSurface.w;
    m.subsurface = uStyle.x;

    // Height above the seam and distance from the axis, the same for either half.
    float height = abs(vLocal.y);
    float radius = length(vLocal.xz);
    float angle = atan(vLocal.z, vLocal.x);
    bool face = height > HalfHeight - 0.01 && radius < DiscRadius - 0.02;
    int pattern = int(uStyle.z + 0.5);
    if (pattern == 1) {
        // Wood cut across the grain: straight lines over the face, rings on the side.
        vec3 q = vLocal + vec3(uDetail.x * 3.1, 0.0, uDetail.x * 1.7);
        float warp = Fbm(q * vec3(2.0, 5.0, 5.0)) * 0.6;
        float rings = fract(length(q.yz + vec2(0.9 + uDetail.x, -0.7)) * 24.0 + warp);
        float late = smoothstep(0.0, 0.12, rings) * (1.0 - smoothstep(0.3, 0.65, rings));
        late = mix(late, 0.3, smoothstep(0.01, 0.03, length(localPixel)));
        float fibers = mix(Noise(vec2(q.x * 30.0, length(q.yz) * 400.0)), 0.5, smoothstep(0.0015, 0.005, length(localPixel)));
        m.albedo *= 1.0 - uStyle.y * (0.24 * late + 0.1 * fibers);
    }
    else if (pattern == 2) {
        // Turned metal: fine circular marks on the face.
        float marks = mix(Noise(vec2(radius * 900.0, angle * 3.0)), 0.5, smoothstep(0.0005, 0.0015, length(localPixel.xz)));
        m.roughness = clamp(m.roughness + (marks - 0.5) * 0.12 * uStyle.y, 0.03, 1.0);
    }
    else if (pattern == 3) {
        // Veined stone.
        vec3 q = vLocal + uDetail.x * 7.0;
        float warp = Fbm(q * 3.5 + 11.0);
        float veins = abs(sin((q.x * 2.0 + q.y * 3.0 + q.z + warp * 3.0) * 4.0));
        float vein = 1.0 - smoothstep(0.0, 0.08, veins);
        m.albedo = mix(m.albedo, m.albedo * 0.55 + vec3(0.12), vein * uStyle.y);
        m.albedo *= 0.94 + 0.12 * Fbm(q * 22.0);
    }

    // The disc placed last carries a small mark in the middle of its face.
    if (uDetail.y > 0.0 && face) {
        float mark = 1.0 - smoothstep(0.055 - 0.7 * radiusPixel, 0.055 + 0.7 * radiusPixel, radius);
        m.albedo = mix(m.albedo, uAccentWarm.rgb * 0.6, mark * uDetail.y);
        m.metallic *= 1.0 - mark * uDetail.y;
        emissive += uAccentWarm.rgb * mark * uDetail.y * 0.35;
    }

    // The ground darkens what faces it, near the bottom most.
    float above = vWorld.y - uStyle.w;
    float ground = mix(0.45, 1.0, smoothstep(0.0, 0.25, above));
    m.occlusion = vOcclusion * mix(1.0, ground, Saturate(0.65 - N.y * 0.65));

    // Neon in the groove round the face and along the seam.
    float faceRing = LineCoverage(abs(radius - Ring), 0.006, radiusPixel) * step(HalfHeight - 0.015, height);
    float seamRing = LineCoverage(height, 0.007, heightPixel) * step(DiscRadius - 0.03, radius);
    m.emissive = emissive + uTrim.rgb * uTrim.a * (faceRing + seamRing);

    m.roughness = WidenRoughness(m.roughness, spread);
    m.clearcoatRoughness = WidenRoughness(m.clearcoatRoughness, spread);
    float shadow = uEye.w > 0.5 ? 1.0 : Shadow(vWorld, N);
    vec3 color = Shade(m, vWorld, N, V, shadow);

    // The glow of a disc that a move would turn: a halo at the silhouette and some light all over.
    float NoV = Saturate(dot(N, V));
    color += uGlow.rgb * uGlow.a * (0.22 + 1.6 * pow(1.0 - NoV, 3.0));

    fragColor = vec4(ApplyFog(color, vWorld), uDetail.z);
}
