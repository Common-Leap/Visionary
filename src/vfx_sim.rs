//! A CPU simulation of the game's effect particles for the 3D preview.
//!
//! The rules follow the game's own effect runtime (`nn::vfx`), read from a title that ships its
//! function names: emission by rate and interval, spawn shapes, velocity from the shape normal
//! and a designated direction, air resistance as `vel *= air ^ frame`, gravity added per frame,
//! and colour, alpha and scale from the same 8-key curves the runtime evaluates. Values come
//! from the `.eff` file through `effect_library`. Random draws use the emitter's own LCG, as the
//! runtime does, so a preview is the same every time it plays.

use effect_library::emitter::{AnimationKeyTable, EmitterData};
use effect_library::ColorType;
use glam::Vec3;

/// One particle as the preview draws it, in the effect's own space.
#[derive(Debug, Clone, Copy)]
pub struct Particle {
    pub position: Vec3,
    /// Linear RGBA, alpha included.
    pub color: [f32; 4],
    /// Half the width of the particle's quad.
    pub size: f32,
    /// Which emitter of the set made it, for its texture and blending.
    pub emitter: usize,
    pub additive: bool,
    /// Motion this frame, for billboards drawn along it.
    pub velocity: Vec3,
    /// Height over width of the quad, for stretched billboards.
    pub stretch: f32,
    /// Billboard type: 0 facing the camera, 3 and 5 along the motion.
    pub billboard: u8,
}

/// The runtime's emitter LCG.
struct Lcg(u32);

impl Lcg {
    /// A float in [0, 1).
    fn next(&mut self) -> f32 {
        let value = self.0;
        self.0 = self.0.wrapping_mul(0x41c6_4e6d).wrapping_add(0x3039);
        value as f32 / 4_294_967_296.0
    }

    /// A float in [-1, 1).
    fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }

    fn unit_vector(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.signed(), self.signed(), self.signed());
            let length = v.length();
            if length > 1e-4 && length <= 1.0 {
                return v / length;
            }
        }
    }
}

struct Live {
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
    scale: Vec3,
    /// The particle's fixed random, for random colours and loop offsets.
    random: f32,
}

/// Evaluate an 8-key table at `t` in [0, 1] (Calculate8KeyAnim): no keys is 1, one key or before
/// the first is the first key, past the last holds it, otherwise linear between keys.
fn eval_keys(table: &AnimationKeyTable, count: u32, t: f32) -> Vec3 {
    let count = (count as usize).min(8);
    let key = |i: usize| Vec3::new(table.keys[i].x, table.keys[i].y, table.keys[i].z);
    if count == 0 {
        return Vec3::ONE;
    }
    if count == 1 || t <= table.keys[0].time {
        return key(0);
    }
    if t >= table.keys[count - 1].time {
        return key(count - 1);
    }
    for i in 0..count - 1 {
        let (a, b) = (table.keys[i].time, table.keys[i + 1].time);
        if t >= a && t < b {
            let f = if b > a { (t - a) / (b - a) } else { 0.0 };
            return key(i).lerp(key(i + 1), f);
        }
    }
    key(count - 1)
}

/// Time along a curve: life-normalised, or wrapped by a loop rate in frames.
fn curve_time(age: f32, life: f32, loop_rate: i32, looping: bool, offset: f32) -> f32 {
    if looping && loop_rate > 0 {
        ((age + offset) % loop_rate as f32) / loop_rate as f32
    } else {
        age / life.max(1.0)
    }
}

/// Spawn position and outward direction for a volume type (the runtime's g_EmitFunctions order:
/// point, circle, circle divided, circle fill, sphere, sphere 32, sphere 64, sphere fill,
/// cylinder, cylinder fill, box, box fill, line, line divided, rectangle, primitive).
fn spawn_shape(data: &EmitterData, rng: &mut Lcg, index: usize, count: usize) -> (Vec3, Vec3) {
    let shape = &data.shape_info;
    let radius = Vec3::new(
        shape.volume_radius_x * shape.volume_form_scale_x.max(0.0),
        shape.volume_radius_y * shape.volume_form_scale_y.max(0.0),
        shape.volume_radius_z * shape.volume_form_scale_z.max(0.0),
    );
    let sweep = if shape.sweep_longitude > 0.0 {
        shape.sweep_longitude
    } else {
        std::f32::consts::TAU
    };
    let circle = |angle: f32, fill: f32| {
        let dir = Vec3::new(angle.sin(), 0.0, angle.cos());
        (dir * radius * fill, dir)
    };
    match shape.volume_type {
        1 | 3 => {
            let angle = shape.sweep_start + rng.next() * sweep;
            let fill = if shape.volume_type == 3 {
                1.0 - shape.caliber_ratio * rng.next()
            } else {
                1.0
            };
            circle(angle, fill)
        }
        2 => circle(
            shape.sweep_start + sweep * index as f32 / count.max(1) as f32,
            1.0,
        ),
        4..=7 => {
            let dir = rng.unit_vector();
            let fill = if shape.volume_type == 7 {
                1.0 - shape.caliber_ratio * rng.next()
            } else {
                1.0
            };
            (dir * radius * fill, dir)
        }
        8 | 9 => {
            let angle = shape.sweep_start + rng.next() * sweep;
            let (mut position, dir) = circle(angle, 1.0);
            position.y = rng.signed() * radius.y;
            (position, dir)
        }
        10 | 11 => {
            let position = Vec3::new(rng.signed(), rng.signed(), rng.signed()) * radius;
            (position, position.normalize_or_zero())
        }
        12 | 13 => {
            let t = if shape.volume_type == 13 {
                index as f32 / count.max(1) as f32 - 0.5
            } else {
                rng.signed() * 0.5
            };
            let position = Vec3::new(0.0, 0.0, shape.line_center + t * shape.line_length);
            (position, Vec3::Z)
        }
        14 => {
            let position = Vec3::new(rng.signed() * radius.x, 0.0, rng.signed() * radius.z);
            (position, Vec3::Y)
        }
        // Point (and primitives, approximated by their centre).
        _ => (Vec3::ZERO, rng.unit_vector()),
    }
}

struct EmitterSim<'a> {
    data: &'a EmitterData,
    rng: Lcg,
    timer: f32,
    interval: f32,
    accumulated: f32,
    particles: Vec<Live>,
}

impl<'a> EmitterSim<'a> {
    fn new(data: &'a EmitterData) -> Self {
        Self {
            data,
            rng: Lcg(data.random_seed ^ 0x5eed),
            timer: f32::MAX,
            interval: 0.0,
            accumulated: 0.0,
            particles: Vec::new(),
        }
    }

    fn spawn(&mut self, count: usize) {
        let data = self.data;
        let velocity = &data.particle_velocity;
        let scale = &data.particle_scale;
        for index in 0..count {
            let (mut position, normal) = spawn_shape(data, &mut self.rng, index, count);
            if data.emission.position_random > 0.0 {
                let jitter =
                    Vec3::new(self.rng.signed(), 0.0, self.rng.signed()).normalize_or_zero();
                position += jitter * data.emission.position_random;
            }
            let mut direction = Vec3::new(
                velocity.designated_dir_x,
                velocity.designated_dir_y,
                velocity.designated_dir_z,
            )
            .normalize_or_zero();
            if velocity.diffusion_dir_angle > 0.0 && direction != Vec3::ZERO {
                let spread = velocity.diffusion_dir_angle.to_radians();
                direction = (direction + self.rng.unit_vector() * spread.sin()).normalize_or_zero();
            }
            let mut vel =
                normal * velocity.all_direction + direction * velocity.designated_dir_scale;
            vel *= 1.0 - self.rng.next() * velocity.vel_random;
            let life_random = data.particle_data.life_random.max(0) as f32;
            let life = if data.particle_data.infinite_life {
                f32::MAX
            } else {
                (data.particle_data.life as f32 - self.rng.next() * life_random).max(1.0)
            };
            let shrink = Vec3::new(
                1.0 - self.rng.next() * scale.scale_random_x,
                1.0 - self.rng.next() * scale.scale_random_y,
                1.0 - self.rng.next() * scale.scale_random_z,
            );
            self.particles.push(Live {
                position,
                velocity: vel,
                age: 0.0,
                life,
                scale: Vec3::new(scale.scale_x, scale.scale_y, scale.scale_z) * shrink,
                random: self.rng.next(),
            });
        }
    }

    /// One frame: emission (TryEmitParticle), then motion (CalculateParticleBehavior).
    fn step(&mut self, frame: f32) {
        let emission = &self.data.emission;
        let start = emission.start as f32;
        let active = frame >= start
            && (emission.duration == 0
                || frame < start + emission.duration as f32
                || emission.is_one_time);
        if emission.is_one_time {
            if frame == start {
                let n = emission.rate * (1.0 - emission.rate_random / 100.0 * self.rng.next());
                self.spawn(n.max(0.0) as usize);
            }
        } else if active {
            if self.timer >= self.interval {
                let rate = emission.rate * (1.0 + emission.rate_random / 100.0 * self.rng.next());
                self.accumulated = (self.accumulated + rate).max(0.0);
                let n = self.accumulated.floor();
                if n > 0.0 {
                    self.spawn(n as usize);
                    self.accumulated -= n;
                    self.timer = 0.0;
                    let jitter = emission.interval_random * self.rng.next();
                    self.interval = (emission.interval as f32 + jitter).max(0.0);
                } else {
                    self.timer = 0.0;
                }
            } else {
                self.timer += 1.0;
            }
        }
        let statics = &self.data.emitter_static;
        let gravity = Vec3::new(
            statics.gravity_dir_x,
            statics.gravity_dir_y,
            statics.gravity_dir_z,
        ) * statics.gravity_scale;
        let air = statics.air_res;
        for particle in &mut self.particles {
            particle.position += particle.velocity;
            if air != 1.0 {
                particle.velocity *= air;
            }
            particle.velocity += gravity;
            particle.age += 1.0;
        }
        self.particles
            .retain(|particle| particle.age < particle.life);
    }

    fn draw(&self, emitter: usize, out: &mut Vec<Particle>) {
        let data = self.data;
        let statics = &data.emitter_static;
        let colors = &data.particle_color;
        let pd = &data.particle_data;
        // Blend types: 0 normal, 1 add, 2 subtract, 3 screen, 4 multiply.
        let additive = matches!(data.render_state.blend_type, 1 | 3);
        for particle in &self.particles {
            let channel = |kind: &ColorType,
                           table: &AnimationKeyTable,
                           count: u32,
                           base: Vec3,
                           rate: i32,
                           looping: bool,
                           loop_random: bool|
             -> Vec3 {
                match kind {
                    ColorType::Animated8Key => {
                        let offset = if loop_random {
                            particle.random * rate.max(1) as f32
                        } else {
                            0.0
                        };
                        eval_keys(
                            table,
                            count,
                            curve_time(particle.age, particle.life, rate, looping, offset),
                        )
                    }
                    ColorType::Random | ColorType::Unknown(3) if count > 0 => {
                        let i = ((particle.random * count as f32) as usize).min(count as usize - 1);
                        let key = &table.keys[i];
                        Vec3::new(key.x, key.y, key.z)
                    }
                    _ => base,
                }
            };
            let rgb = channel(
                &colors.color0_type,
                &statics.color0,
                statics.num_color0_keys,
                Vec3::new(colors.color0_r, colors.color0_g, colors.color0_b),
                pd.color0_loop_rate,
                pd.loop_color0,
                pd.loop_random_color0,
            ) * statics.color_scale.max(0.0);
            let alpha = channel(
                &colors.alpha0_type,
                &statics.alpha0,
                statics.num_alpha0_keys,
                Vec3::splat(colors.alpha0),
                pd.alpha0_loop_rate,
                pd.loop_alpha0,
                pd.loop_random_alpha0,
            )
            .x;
            let scale_curve = if statics.num_scale_keys >= 2 {
                eval_keys(
                    &statics.scale_anim,
                    statics.num_scale_keys,
                    curve_time(
                        particle.age,
                        particle.life,
                        pd.scale_loop_rate,
                        pd.scale_loop,
                        0.0,
                    ),
                )
            } else if statics.num_scale_keys == 1 {
                eval_keys(&statics.scale_anim, 1, 0.0)
            } else {
                Vec3::ONE
            };
            let scale = particle.scale * scale_curve;
            out.push(Particle {
                position: particle.position,
                color: [rgb.x, rgb.y, rgb.z, alpha.clamp(0.0, 1.0)],
                size: scale.x.abs(),
                emitter,
                additive,
                velocity: particle.velocity,
                stretch: if scale.x.abs() > 1e-4 {
                    scale.y.abs() / scale.x.abs()
                } else {
                    1.0
                },
                billboard: pd.billboard_type,
            });
        }
    }
}

/// Every particle of an emitter set `frame` frames after it was spawned. The simulation runs
/// from the spawn each time, so a scrubbed preview always matches a played one.
pub fn particles_at(emitters: &[EmitterData], frame: u32) -> Vec<Particle> {
    let mut sims: Vec<EmitterSim> = emitters.iter().map(EmitterSim::new).collect();
    for f in 0..=frame {
        for sim in &mut sims {
            sim.step(f as f32);
        }
    }
    let mut out = Vec::new();
    for (index, sim) in sims.iter().enumerate() {
        sim.draw(index, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_the_runtime_rules() {
        let mut bytes = vec![0u8; 8 * 16];
        for (i, value) in [0.0f32, 0.0, 0.0, 0.25, 1.0, 2.0, 3.0, 0.75]
            .iter()
            .enumerate()
        {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        let table = AnimationKeyTable::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(eval_keys(&table, 0, 0.5), Vec3::ONE);
        assert_eq!(eval_keys(&table, 2, 0.0), Vec3::ZERO);
        assert_eq!(eval_keys(&table, 2, 1.0), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(eval_keys(&table, 2, 0.5), Vec3::new(0.5, 1.0, 1.5));
    }

    #[test]
    fn the_lcg_is_the_runtimes() {
        let mut rng = Lcg(1);
        rng.next();
        assert_eq!(rng.0, 0x41c6_4e6d_u32.wrapping_add(0x3039));
    }
}

/// One emitter set's root emitters, with the pool index of each one's first texture.
pub struct SimSet {
    pub emitters: Vec<EmitterData>,
    pub textures: Vec<Option<usize>>,
    /// The emitter's colour comes from its curves alone and the texture only shapes alpha
    /// (combiner colour process 2).
    pub alpha_only: Vec<bool>,
}

/// An `.eff` file's sets by entry name (as written and lower-cased), and its texture pool.
pub struct SimFile {
    pub sets: std::collections::HashMap<String, std::sync::Arc<SimSet>>,
    pub pool: Option<std::sync::Arc<Vec<u8>>>,
    pub texture_names: Vec<String>,
}

pub fn load_sets(path: &std::path::Path) -> anyhow::Result<SimFile> {
    let bytes = std::fs::read(path)?;
    let file = effect_library::NamcoEffectFile::load(&bytes)?;
    let mut out = SimFile {
        sets: Default::default(),
        pool: None,
        texture_names: Vec::new(),
    };
    let Some(ptcl) = file.ptcl_file.as_ref() else {
        return Ok(out);
    };
    let descriptors = ptcl
        .texture_info
        .as_ref()
        .map(|info| info.descriptors.as_slice())
        .unwrap_or_default();
    out.texture_names = descriptors.iter().map(|d| d.name.clone()).collect();
    out.pool = ptcl
        .texture_info
        .as_ref()
        .and_then(|info| info.binary_data.clone())
        .map(std::sync::Arc::new);
    let index = crate::effects::load_effect(path)?.index;
    for (name, set) in index.handles {
        let Some(set) = usize::try_from(set)
            .ok()
            .and_then(|set| ptcl.emitter_list.emitter_sets.get(set))
        else {
            continue;
        };
        let emitters: Vec<EmitterData> = set
            .emitters
            .iter()
            .map(|emitter| emitter.data.clone())
            .collect();
        let textures = emitters
            .iter()
            .map(|data| {
                let id = data.sampler0.as_ref()?.texture_id;
                descriptors.iter().position(|texture| texture.id == id)
            })
            .collect();
        let alpha_only = emitters
            .iter()
            .map(|data| {
                matches!(
                    &data.combiner,
                    Some(effect_library::emitter::EmitterCombinerVariant::Legacy(c))
                        if c.color_combiner_process == 2
                )
            })
            .collect();
        out.sets.insert(
            name,
            std::sync::Arc::new(SimSet {
                emitters,
                textures,
                alpha_only,
            }),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod corpus_tests {
    /// Prints a set's emitters. VISIONARY_DATA_ROOT and VISIONARY_VFX_SET (eff path, set name).
    #[test]
    fn describe_a_set() {
        let (Some(root), Some(spec)) = (
            std::env::var_os("VISIONARY_DATA_ROOT"),
            std::env::var("VISIONARY_VFX_SET").ok(),
        ) else {
            return;
        };
        let (file, name) = spec.split_once(':').unwrap();
        let sets = super::load_sets(&std::path::Path::new(&root).join(file)).unwrap();
        for data in sets.sets[name].emitters.iter() {
            eprintln!(
                "{:?} vol={} bill={} blend={} life={}+-{} rate={} int={} one={} scale=({},{}) keys={} col0={:?} a0={:?} allDir={} dir=({},{},{})*{} grav={} air={} prim={}",
                data.name, data.shape_info.volume_type, data.particle_data.billboard_type,
                data.render_state.blend_type, data.particle_data.life, data.particle_data.life_random,
                data.emission.rate, data.emission.interval, data.emission.is_one_time,
                data.particle_scale.scale_x, data.particle_scale.scale_y,
                data.emitter_static.num_scale_keys, data.particle_color.color0_type,
                data.particle_color.alpha0_type, data.particle_velocity.all_direction,
                data.particle_velocity.designated_dir_x, data.particle_velocity.designated_dir_y,
                data.particle_velocity.designated_dir_z, data.particle_velocity.designated_dir_scale,
                data.emitter_static.gravity_scale, data.emitter_static.air_res,
                data.particle_data.primitive_id,
            );
            if let Some(effect_library::emitter::EmitterCombinerVariant::Legacy(c)) = &data.combiner
            {
                eprintln!("   combiner cp={} ap={} t1c={} t1a={} ci=({},{},{}) ai=({},{},{}) shader={} apply_alpha={}",
                    c.color_combiner_process, c.alpha_combiner_process, c.texture1_color_blend, c.texture1_alpha_blend,
                    c.tex_color0_input_type, c.tex_color1_input_type, c.tex_color2_input_type,
                    c.tex_alpha0_input_type, c.tex_alpha1_input_type, c.tex_alpha2_input_type, c.shader_type, c.apply_alpha);
            } else {
                eprintln!("   combiner {:?}", data.combiner.as_ref().map(|_| "other"));
            }
            eprintln!(
                "   color0 {:?} scale {}",
                (
                    data.particle_color.color0_r,
                    data.particle_color.color0_g,
                    data.particle_color.color0_b
                ),
                data.emitter_static.color_scale
            );
            let k = |t: &super::AnimationKeyTable, n: u32| {
                t.keys[..n.min(8) as usize]
                    .iter()
                    .map(|k| format!("({:.2},{:.2},{:.2}@{:.2})", k.x, k.y, k.z, k.time))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            eprintln!(
                "   c0keys {}",
                k(
                    &data.emitter_static.color0,
                    data.emitter_static.num_color0_keys
                )
            );
            eprintln!(
                "   skeys {}",
                k(
                    &data.emitter_static.scale_anim,
                    data.emitter_static.num_scale_keys
                )
            );
        }
    }
}
