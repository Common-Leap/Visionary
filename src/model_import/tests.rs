use super::*;

/// A square of two triangles, textured, tinted and half clear at one corner.
fn square() -> Model {
    let corner = |x: f32, y: f32, alpha: f32| Vertex {
        position: [x * 10.0, y * 10.0, 0.5],
        normal: [0.0, 0.0, 1.0],
        uv: [x * 2.0, 1.0 - y],
        uv2: None,
        colour: [1.0, 0.5, 0.25, alpha],
    };
    Model {
        vertices: vec![
            corner(0.0, 0.0, 0.5),
            corner(1.0, 0.0, 1.0),
            corner(1.0, 1.0, 1.0),
            corner(0.0, 1.0, 1.0),
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
    }
}

fn near(a: &[f32], b: &[f32], by: f32) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() <= by)
}

fn same(a: &Model, b: &Model, by: f32) {
    assert_eq!(a.triangles.len(), b.triangles.len());
    // Vertices can be in another order: compared corner by corner.
    for (ta, tb) in a.triangles.iter().zip(&b.triangles) {
        for (ia, ib) in ta.iter().zip(tb) {
            let (va, vb) = (&a.vertices[*ia as usize], &b.vertices[*ib as usize]);
            assert!(near(&va.position, &vb.position, by * 10.0), "{va:?} {vb:?}");
            assert!(near(&va.normal, &vb.normal, by.max(5e-3)), "{va:?} {vb:?}");
            assert!(near(&va.uv, &vb.uv, by), "{va:?} {vb:?}");
            assert!(near(&va.colour, &vb.colour, by.max(5e-3)), "{va:?} {vb:?}");
        }
    }
}

#[test]
fn halves_round_trip() {
    for v in [
        0.0, 1.0, -1.0, 0.1, 3.75, -12.5, 1e-6, 2048.0, 0.99999, 60000.0,
    ] {
        let back = half_to_f32(f32_to_half(v));
        assert!((back - v).abs() <= v.abs() * 1e-3 + 1e-7, "{v} {back}");
    }
    assert!(half_to_f32(f32_to_half(1e9)).is_finite());
}

#[test]
fn a_glb_comes_back_as_it_was() {
    let mut model = square();
    let back = glb::read(&glb::write("square", &model)).unwrap();
    same(&model, &back, 0.0);
    assert!(back.vertices.iter().all(|v| v.uv2.is_none()));
    for v in &mut model.vertices {
        v.uv2 = Some([v.uv[1], v.uv[0]]);
    }
    let back = glb::read(&glb::write("square", &model)).unwrap();
    assert_eq!(back, model);
    assert!(glb::read(b"not a model").is_err());
    let cut = glb::write("square", &model);
    assert!(glb::read(&cut[..cut.len() - 8]).is_err());
}

#[test]
fn an_obj_comes_back_without_its_opacity() {
    let model = square();
    let mut want = model.clone();
    want.vertices.iter_mut().for_each(|v| v.colour[3] = 1.0);
    same(
        &want,
        &obj::read(&obj::write("square", &model)).unwrap(),
        0.0,
    );
}

#[test]
fn an_obj_from_another_program_reads() {
    // A four-cornered face, corners counted back from the last, no normals, and comments.
    let text =
        "# made elsewhere\nmtllib x.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\n\
                vt 1 1\nvt 0 1\nusemtl a\nf -4/1 -3/2 -2/3 -1/4\n";
    let model = obj::read(text).unwrap();
    assert_eq!((model.vertices.len(), model.triangles.len()), (4, 2));
    assert!(model
        .vertices
        .iter()
        .all(|v| near(&v.normal, &[0.0, 0.0, 1.0], 1e-6)));
    assert_eq!(model.vertices[0].uv, [0.0, 1.0]);
    for bad in ["f 1 2 3", "v 1 2", "v 0 0 x", "v 0 0 0\nf 1/9 1 1"] {
        assert!(obj::read(bad).is_err(), "{bad}");
    }
}

/// A node's place, turn and mirroring are taken in, as a program that moves its meshes by their
/// nodes writes them.
#[test]
fn a_glb_is_read_where_its_nodes_put_it() {
    let bytes = glb::write("square", &square());
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(
        text.contains(r#""nodes":[{"mesh":0,"name":"square"}]"#),
        "{text}"
    );
    let size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let mut json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + size]).unwrap();
    json["nodes"][0]["scale"] = serde_json::json!([-2.0, 1.0, 1.0]);
    json["nodes"][0]["translation"] = serde_json::json!([0.0, 5.0, 0.0]);
    let mut document = json.to_string().into_bytes();
    document.resize(document.len().next_multiple_of(4), b' ');
    let mut moved = bytes[..8].to_vec();
    let rest = &bytes[20 + size..];
    moved.extend(((20 + document.len() + rest.len()) as u32).to_le_bytes());
    moved.extend((document.len() as u32).to_le_bytes());
    moved.extend(&bytes[16..20]);
    moved.extend(document);
    moved.extend(rest);
    let model = glb::read(&moved).unwrap();
    let corner = &model.vertices[model.triangles[0][0] as usize];
    assert_eq!(corner.position, [0.0, 5.0, 0.5]);
    let far = model
        .vertices
        .iter()
        .map(|v| v.position[0])
        .fold(0.0, f32::min);
    assert_eq!(far, -20.0);
    // Still facing the way its normals do.
    let [a, b, c] =
        model.triangles[0].map(|i| glam::Vec3::from(model.vertices[i as usize].position));
    assert!((b - a).cross(c - a).z > 0.0);
}

fn pool(file: &str) -> Option<(Vec<u8>, ModelPool)> {
    let Some(root) = std::env::var_os("VISIONARY_EFF_ROOT") else {
        eprintln!("skipped: set VISIONARY_EFF_ROOT to the extracted effect/ tree");
        return None;
    };
    let bytes = std::fs::read(std::path::Path::new(&root).join(file)).unwrap();
    let eff = effect_library::NamcoEffectFile::load(&bytes).unwrap();
    let pool = ModelPool::of(eff.ptcl_file.as_ref()?)?;
    Some((bytes, pool))
}

/// Every model of a file comes out and goes back in as itself, in every vertex layout.
#[test]
fn the_games_models_go_out_and_back() {
    let mut layouts = std::collections::BTreeSet::new();
    for file in ["mario", "edge", "ryu", "bayonetta", "link"] {
        let Some((_, pool)) = pool(&format!("effect/fighter/{file}/ef_{file}.eff")) else {
            return;
        };
        for info in pool.infos() {
            let model = pool.model(info.id).unwrap();
            assert_eq!(model.vertices.len(), info.vertices);
            assert_eq!(model.triangles.len(), info.triangles);
            assert!(model
                .triangles
                .iter()
                .flatten()
                .all(|i| (*i as usize) < info.vertices));
            let back = glb::read(&glb::write(&info.name, &model)).unwrap();
            same(&model, &back, 1e-3);
            let binary = replace(&pool.binary, &pool.descriptors, info.id, &back).unwrap();
            let after = read(&binary, &pool.descriptors, info.id).unwrap();
            same(&model, &after, 1e-3);
            layouts.insert(info.attributes.clone());
        }
    }
    assert!(layouts.len() > 1, "{layouts:?}");
}

/// A replaced model is in the written file, which keeps every rule the game's files keep, and
/// every other model is as it was.
#[test]
fn a_replaced_model_is_in_the_exported_file() {
    const MARIO: &str = "effect/fighter/mario/ef_mario.eff";
    let Some((bytes, before)) = pool(MARIO) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("square.glb");
    save(&path, "square", &square()).unwrap();
    let info = before.infos()[0].clone();
    let eff = crate::mod_project::EffMod {
        source_rel: MARIO.into(),
        models: vec![crate::mod_project::ModelImport {
            model_id: info.id,
            name: info.name.clone(),
            path: path.to_string_lossy().into_owned(),
        }],
        ..Default::default()
    };
    let out = crate::eff_export::rebuild_eff_bytes(&bytes, &eff, None).unwrap();
    let eff = effect_library::NamcoEffectFile::load(&out).unwrap();
    let after = ModelPool::of(eff.ptcl_file.as_ref().unwrap()).unwrap();
    same(&square(), &after.model(info.id).unwrap(), 1e-3);
    assert_eq!(after.info(info.id).unwrap().attributes, info.attributes);
    for other in &before.infos()[1..] {
        assert_eq!(after.info(other.id), Some(other));
        assert_eq!(
            before.model(other.id).unwrap(),
            after.model(other.id).unwrap()
        );
    }
    // And the preview's own reader agrees.
    let meshes = eff_wgpu::mesh::meshes(eff.ptcl_file.as_ref().unwrap());
    assert_eq!(meshes[&info.id].vertices.len(), 4);
    assert_eq!(meshes[&info.id].indices.len(), 6);
}

#[test]
fn what_can_not_be_a_model_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name);
    std::fs::write(path("a.fbx"), b"x").unwrap();
    std::fs::write(path("a.obj"), b"v 0 0 0\n").unwrap();
    assert!(load(&path("a.fbx")).is_err());
    assert!(load(&path("a.obj")).is_err(), "no triangles");
    assert!(load(&path("gone.obj")).is_err());
    let Some((_, pool)) = pool("effect/fighter/mario/ef_mario.eff") else {
        return;
    };
    let mut big = square();
    big.vertices = vec![big.vertices[0]; 70_000];
    let id = pool.infos()[0].id;
    assert!(pool.check(id, &big).is_err());
    assert!(pool.check(id ^ 1, &square()).is_err());
    assert_eq!(pool.check(id, &square()).unwrap().triangles, 2);
}
