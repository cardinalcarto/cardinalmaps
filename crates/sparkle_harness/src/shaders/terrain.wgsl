// Terrain tiles: `sparkle::terrain::TerrainMesh` over the tile, lifted by the DEM (see
// `elevation`) and textured with the layers rendered for the tile, then fogged with
// distance.
//
// `extra[0].x`: how far skirts hang below the edge, in logical pixels.

@group(1) @binding(0) var drape: texture_2d<f32>;
@group(1) @binding(1) var drape_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fog: f32,
};

@vertex
fn vs_main(@location(0) uv: vec2<f32>, @location(1) skirt: f32) -> VertexOut {
    let pos = uv * draw.tile.w;
    let ground = elevation(pos);
    var height = ground - skirt * draw.extra[0].x;
    var flat = draw.matrix * vec4<f32>(pos, height, 1.0);
    var position = flat;
    let t = draw.globe.x;
    if t > 0.0 {
        var world = tile_world(pos);
        // The skirts along the top of the world's first row and the bottom of its last
        // reach to the poles instead, closing the globe where Mercator ends. On the plane
        // they fold up into the edge.
        let n = draw.tile.z;
        let top = draw.tile.y == 0.0 && uv.y == 0.0;
        let bottom = draw.tile.y == n - 1.0 && uv.y == 1.0;
        if skirt > 0.5 && (top || bottom) {
            world.y = select(2.0, -1.0, top);
            height = 0.0;
            flat = draw.matrix * vec4<f32>(pos, ground, 1.0);
        }
        let sphere = draw.view * vec4<f32>(globe_position_of(world, height), 1.0);
        position = mix(flat, sphere, t);
    }
    var out: VertexOut;
    out.position = position;
    out.uv = uv;
    out.fog = fog_amount(position.w);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(drape, drape_sampler, in.uv);
    return mix(color, draw.fog_color * color.a, in.fog);
}
