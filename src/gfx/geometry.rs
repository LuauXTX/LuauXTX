use mlua::{Error, Result, Table};

pub type Point = [f32; 2];
pub type Color = [f32; 4];
pub fn error(message: impl Into<String>) -> Error {
    Error::RuntimeError(message.into())
}
pub fn finite(value: f32, name: &str) -> Result<f32> {
    if value.is_finite() && value.abs() <= 1e7 {
        Ok(value)
    } else {
        Err(error(format!(
            "{name} must be finite and within +/-10000000"
        )))
    }
}
pub fn number(table: &Table, key: &str, default: f32) -> Result<f32> {
    finite(table.get::<Option<f32>>(key)?.unwrap_or(default), key)
}
pub fn positive(value: f32, name: &str, limit: f32) -> Result<f32> {
    if value.is_finite() && value > 0. && value <= limit {
        Ok(value)
    } else {
        Err(error(format!("{name} must be > 0 and <= {limit}")))
    }
}
pub fn color(table: Option<Table>) -> Result<Color> {
    let Some(table) = table else {
        return Ok([1.; 4]);
    };
    let mut result = [1.; 4];
    for (i, slot) in result.iter_mut().enumerate() {
        let value = table
            .get::<Option<f32>>(i + 1)?
            .unwrap_or(if i == 3 { 255. } else { 0. });
        if !value.is_finite() || !(0. ..=255.).contains(&value) {
            return Err(error("color channels must be in 0..255"));
        }
        *slot = value / 255.;
    }
    Ok(result)
}
pub fn linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Transform(pub [f32; 6]);
impl Transform {
    pub const IDENTITY: Self = Self([1., 0., 0., 1., 0., 0.]);
    pub fn point(self, p: Point) -> Point {
        let [a, b, c, d, x, y] = self.0;
        [a * p[0] + c * p[1] + x, b * p[0] + d * p[1] + y]
    }
    pub fn then(self, rhs: Self) -> Self {
        let [a, b, c, d, x, y] = self.0;
        let [e, f, g, h, u, v] = rhs.0;
        Self([
            a * e + c * f,
            b * e + d * f,
            a * g + c * h,
            b * g + d * h,
            a * u + c * v + x,
            b * u + d * v + y,
        ])
    }
    pub fn from_options(t: &Table) -> Result<Self> {
        let angle = number(t, "rotation", 0.)?;
        let (s, c) = angle.sin_cos();
        let sx = number(t, "scaleX", 1.)?;
        let sy = number(t, "scaleY", 1.)?;
        let result = Self([
            c * sx,
            s * sx,
            -s * sy,
            c * sy,
            number(t, "x", 0.)?,
            number(t, "y", 0.)?,
        ]);
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub position: Point,
    pub uv: Point,
}
fn cross(a: Point, b: Point, c: Point) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
pub fn area(p: &[Point]) -> f32 {
    (0..p.len())
        .map(|i| {
            let a = p[i];
            let b = p[(i + 1) % p.len()];
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f32>()
        * 0.5
}
// Convex polygon clipping interpolates UVs too, so rotated clips work on text
// and images without requiring a stencil buffer or a tessellation dependency.
pub fn clip(mut input: Vec<Vertex>, polygon: &[Point]) -> Vec<Vertex> {
    if polygon.len() < 3 || area(polygon).abs() < 1e-6 {
        return vec![];
    }
    let orientation = area(polygon).signum();
    for i in 0..polygon.len() {
        if input.is_empty() {
            break;
        }
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let mut output = Vec::new();
        let mut prev = input[input.len() - 1];
        for current in &input {
            let dp = cross(a, b, prev.position) * orientation;
            let dc = cross(a, b, current.position) * orientation;
            if (dp >= 0.) != (dc >= 0.) {
                let t = dp / (dp - dc);
                output.push(Vertex {
                    position: [
                        prev.position[0] + t * (current.position[0] - prev.position[0]),
                        prev.position[1] + t * (current.position[1] - prev.position[1]),
                    ],
                    uv: [
                        prev.uv[0] + t * (current.uv[0] - prev.uv[0]),
                        prev.uv[1] + t * (current.uv[1] - prev.uv[1]),
                    ],
                });
            }
            if dc >= 0. {
                output.push(*current);
            }
            prev = *current;
        }
        input = output;
    }
    input
}
pub fn quad(t: Transform, x: f32, y: f32, w: f32, h: f32) -> Vec<Vertex> {
    [[0., 0.], [1., 0.], [1., 1.], [0., 1.]]
        .iter()
        .map(|uv| Vertex {
            position: t.point([x + uv[0] * w, y + uv[1] * h]),
            uv: *uv,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transforms_compose_and_clipping_interpolates_uvs() {
        let t = Transform([2., 0., 0., 3., 10., 20.]);
        assert_eq!(
            t.then(Transform([1., 0., 0., 1., 4., 5.])).point([1., 1.]),
            [20., 38.]
        );
        let vertices = clip(
            quad(Transform::IDENTITY, 0., 0., 100., 100.),
            &[[25., 0.], [75., 0.], [75., 100.], [25., 100.]],
        );
        assert_eq!(vertices.len(), 4);
        for v in vertices {
            assert!((25. ..=75.).contains(&v.position[0]));
            assert!((v.uv[0] - v.position[0] / 100.).abs() < 1e-6);
        }
        assert!(clip(quad(t, 0., 0., 1., 1.), &[[0., 0.]; 4]).is_empty());
    }
    #[test]
    fn rejects_non_finite_values() {
        assert!(finite(f32::NAN, "x").is_err());
        assert!(positive(0., "width", 16384.).is_err());
    }
}
