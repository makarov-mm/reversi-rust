// Depth only; a dissolving disc casts its remaining part.

in vec3 vLocal;

uniform float uDissolve;
uniform float uSeed;

void main()
{
    if (uDissolve > 0.0 && Fbm(vLocal * 7.0 + 3.0 + uSeed * 5.0) < uDissolve * 1.2 - 0.1)
        discard;
}
