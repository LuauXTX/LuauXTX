struct Input {
 @location(0) position: vec2<f32>, @location(1) uv: vec2<f32>,
 @location(2) color: vec4<f32>, @location(3) size: vec2<f32>,
 @location(4) radius: f32, @location(5) border: f32, @location(6) textured: f32,
};
struct Output {
 @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>,
 @location(1) color: vec4<f32>, @location(2) size: vec2<f32>,
 @location(3) radius: f32, @location(4) border: f32, @location(5) textured: f32,
};
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@vertex fn vs(v: Input) -> Output {
 var out: Output; out.position=vec4(v.position,0.,1.); out.uv=v.uv;
 out.color=v.color; out.size=v.size; out.radius=v.radius; out.border=v.border; out.textured=v.textured; return out;
}
@fragment fn fs(v: Output) -> @location(0) vec4<f32> {
 let q=abs((v.uv-vec2(0.5))*v.size)-(v.size*0.5-vec2(v.radius));
 let distance=length(max(q,vec2(0.)) )+min(max(q.x,q.y),0.)-v.radius;
 let aa=max(fwidth(distance),0.001);
 if v.textured > 0.5 { return textureSampleLevel(image,image_sampler,v.uv,0.)*v.color; }
 var coverage=1.-smoothstep(-aa*0.5,aa*0.5,distance);
 if v.border > 0. { coverage*=smoothstep(-v.border-aa*0.5,-v.border+aa*0.5,distance); }
 return vec4(v.color.rgb,v.color.a*coverage);
}
