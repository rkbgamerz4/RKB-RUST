//! Blockstate and model JSON resolution for user-supplied resource packs.
//!
//! Resolution follows vanilla's rules: pick a blockstate variant (or evaluate
//! multipart conditions), load the model and its parent chain, merge texture
//! aliases, then convert model elements (0..16 model units, `from`/`to`, face
//! `uv`, `cullface`, `rotation`, `tintindex`) into renderer geometry. Model and
//! element rotations are baked into an affine transform the mesher applies.
//!
//! Anything that cannot be resolved falls back to the built-in shape for that
//! block and is counted, so pack coverage is measurable instead of silently
//! wrong. No asset is bundled: every byte comes from the pack the user opened.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use rkb_mc::BlockState;
use serde_json::Value;

use crate::model::{
    block_short_name, builtin_shape, is_approximate_fallback, is_cross_block, render_layer,
    BlockModelSource, Element, ElementFace, ElementTransform, FaceTexture, RenderLayer,
    ResolvedShape, TintKind,
};
use crate::textures::FaceDir;

use super::atlas::TextureStore;
use super::{blockstate_relative, model_relative, texture_relative, PackError, ResourcePack};

/// Maximum parent-chain depth before a model cycle is treated as missing.
const MAX_PARENTS: usize = 16;

#[derive(Clone, Debug)]
struct CachedShape {
    shape: ResolvedShape,
    approximate: bool,
}

/// A resource pack resolved into block shapes and textures.
#[derive(Debug)]
pub struct PackModels {
    pack: ResourcePack,
    store: RefCell<TextureStore>,
    shapes: RefCell<HashMap<BlockState, CachedShape>>,
    blockstates: RefCell<HashMap<String, Option<Value>>>,
    models: RefCell<HashMap<String, Option<Rc<RawModel>>>>,
    /// Unique block states with no resolvable model.
    unmodelled: Cell<usize>,
    /// Texture references that could not be read or decoded.
    missing_textures: Cell<usize>,
    errors: RefCell<Vec<String>>,
}

impl PackModels {
    #[must_use]
    pub fn new(pack: ResourcePack) -> Self {
        Self {
            pack,
            store: RefCell::new(TextureStore::new()),
            shapes: RefCell::new(HashMap::new()),
            blockstates: RefCell::new(HashMap::new()),
            models: RefCell::new(HashMap::new()),
            unmodelled: Cell::new(0),
            missing_textures: Cell::new(0),
            errors: RefCell::new(Vec::new()),
        }
    }

    /// The pack these models came from.
    #[must_use]
    pub fn pack(&self) -> &ResourcePack {
        &self.pack
    }

    /// Number of decoded pack textures (including the missing tile).
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.store.borrow().len()
    }

    /// Generation of the texture set, used to refresh the atlas.
    #[must_use]
    pub fn texture_generation(&self) -> u64 {
        self.store.borrow().generation()
    }

    /// Unique block states drawn with a full-cube fallback.
    #[must_use]
    pub fn unmodelled_blocks(&self) -> usize {
        self.unmodelled.get()
    }

    /// Texture references that fell back to the missing-texture tile.
    #[must_use]
    pub fn missing_textures(&self) -> usize {
        self.missing_textures.get()
    }

    /// First resolution error, for status reporting.
    pub fn first_error(&self) -> Option<String> {
        self.errors.borrow().first().cloned()
    }

    /// Borrow the texture store (for atlas rebuilds).
    pub(crate) fn store(&self) -> std::cell::Ref<'_, TextureStore> {
        self.store.borrow()
    }

    fn note(&self, error: PackError) {
        let mut errors = self.errors.borrow_mut();
        if errors.len() < 8 {
            errors.push(error.to_string());
        }
    }

    fn entry(&self, state: &BlockState) -> CachedShape {
        if let Some(cached) = self.shapes.borrow().get(state) {
            return cached.clone();
        }
        let cached = self.compute(state);
        self.shapes
            .borrow_mut()
            .insert(state.clone(), cached.clone());
        cached
    }

    fn compute(&self, state: &BlockState) -> CachedShape {
        let short = block_short_name(&state.name);
        if let Some(blockstate) = self.blockstate(short) {
            if let Some(applications) = applications(&blockstate, state) {
                if let Some(built) = self.build(short, &applications) {
                    return built;
                }
            }
        }
        self.unmodelled.set(self.unmodelled.get().saturating_add(1));
        CachedShape {
            shape: builtin_shape(state),
            approximate: is_approximate_fallback(state),
        }
    }

    fn blockstate(&self, short: &str) -> Option<Value> {
        let key = short.to_owned();
        if let Some(hit) = self.blockstates.borrow().get(&key) {
            return hit.clone();
        }
        let parsed = match self.pack.read_json(&blockstate_relative(short)) {
            Ok(value) => value,
            Err(error) => {
                self.note(error);
                None
            }
        };
        self.blockstates.borrow_mut().insert(key, parsed.clone());
        parsed
    }

    fn build(&self, short: &str, applications: &[(String, f32, f32)]) -> Option<CachedShape> {
        let mut elements = Vec::new();
        let mut used: Vec<u32> = Vec::new();
        for (model_name, x, y) in applications {
            let Some(model) = self.merged_model(model_name) else {
                continue;
            };
            let block_rot = block_rotation(*x, *y);
            let built = self.elements_for(&model, short, block_rot, &mut used);
            elements.extend(built);
        }
        if elements.is_empty() {
            return None;
        }

        let double_sided = is_cross_block(short);
        if double_sided {
            for element in &mut elements {
                element.double_sided = true;
            }
        }

        let mut layer = render_layer(short);
        if layer == RenderLayer::Opaque {
            let store = self.store.borrow();
            if used.iter().any(|&id| store.has_alpha(id)) {
                layer = RenderLayer::Cutout;
            }
        }

        Some(CachedShape {
            shape: crate::model::ResolvedShape {
                shape: crate::model::BlockShape::Elements(elements),
                layer,
            },
            approximate: false,
        })
    }

    fn elements_for(
        &self,
        model: &RawModel,
        short: &str,
        block_rot: Option<ElementTransform>,
        used: &mut Vec<u32>,
    ) -> Vec<Element> {
        let Some(raw_elements) = &model.elements else {
            return Vec::new();
        };
        let tint = tint_kind(short);
        let mut elements = Vec::with_capacity(raw_elements.len());
        for raw in raw_elements {
            let element_rot = raw
                .rotation
                .map(|(axis, angle, origin)| ElementTransform::rotation(axis, angle, origin));
            let transform = match (block_rot, element_rot) {
                (Some(block), Some(element)) => Some(block.then(element)),
                (Some(block), None) => Some(block),
                (None, Some(element)) => Some(element),
                (None, None) => None,
            };
            let mut faces: [Option<ElementFace>; 6] = [None; 6];
            for face in FaceDir::ALL {
                let Some(raw_face) = raw.faces.get(face_name(face)) else {
                    continue;
                };
                let reference = resolve_texture_ref(model, &raw_face.texture)
                    .unwrap_or_else(|| "__rkb_missing__".to_owned());
                let key = texture_relative(&reference);
                let loaded = match self.pack.texture(&reference) {
                    Ok(image) => image,
                    Err(error) => {
                        self.note(error);
                        None
                    }
                };
                if loaded.is_none() {
                    self.missing_textures
                        .set(self.missing_textures.get().saturating_add(1));
                }
                let mut store = self.store.borrow_mut();
                let id = store.get_or_insert(&key, || loaded);
                drop(store);
                if !used.contains(&id) {
                    used.push(id);
                }
                faces[face.index()] = Some(ElementFace {
                    texture: FaceTexture::Atlas(id),
                    uv: raw_face.uv,
                    cullface: raw_face.cullface,
                    rotation: raw_face.rotation,
                    tint: if raw_face.tintindex.is_some() {
                        tint
                    } else {
                        TintKind::None
                    },
                });
            }
            if faces.iter().all(Option::is_none) {
                continue;
            }
            elements.push(Element {
                from: raw.from,
                to: raw.to,
                faces,
                transform,
                double_sided: false,
            });
        }
        elements
    }

    fn raw_model(&self, name: &str) -> Option<Rc<RawModel>> {
        let key = normalize_model_name(name);
        if let Some(hit) = self.models.borrow().get(&key) {
            return hit.clone();
        }
        let parsed = match self.pack.read_json(&model_relative(&key)) {
            Ok(Some(value)) => Some(Rc::new(parse_model(&value))),
            Ok(None) => None,
            Err(error) => {
                self.note(error);
                None
            }
        };
        self.models.borrow_mut().insert(key, parsed.clone());
        parsed
    }

    fn merged_model(&self, name: &str) -> Option<Rc<RawModel>> {
        let mut chain = Vec::new();
        let mut current = name.to_owned();
        for _ in 0..MAX_PARENTS {
            let model = self.raw_model(&current)?;
            let parent = model.parent.clone();
            chain.push(model);
            match parent {
                Some(parent) => current = parent,
                None => break,
            }
        }
        let mut merged = RawModel::default();
        // Ancestors first so the child overrides textures and elements.
        for model in chain.iter().rev() {
            for (key, value) in &model.textures {
                merged.textures.insert(key.clone(), value.clone());
            }
            if let Some(elements) = &model.elements {
                merged.elements = Some(elements.clone());
            }
        }
        Some(Rc::new(merged))
    }
}

impl BlockModelSource for PackModels {
    fn resolve(&self, state: &BlockState) -> ResolvedShape {
        self.entry(state).shape.clone()
    }

    fn approximate(&self, state: &BlockState) -> bool {
        self.entry(state).approximate
    }
}

#[derive(Debug, Default, Clone)]
struct RawModel {
    parent: Option<String>,
    textures: BTreeMap<String, String>,
    elements: Option<Vec<RawElement>>,
}

#[derive(Debug, Clone)]
struct RawElement {
    from: [f32; 3],
    to: [f32; 3],
    rotation: Option<(u8, f32, [f32; 3])>,
    faces: BTreeMap<String, RawFace>,
}

#[derive(Debug, Clone)]
struct RawFace {
    texture: String,
    uv: Option<[f32; 4]>,
    cullface: Option<FaceDir>,
    rotation: u8,
    tintindex: Option<i32>,
}

fn parse_model(value: &Value) -> RawModel {
    let mut model = RawModel {
        parent: value
            .get("parent")
            .and_then(Value::as_str)
            .map(str::to_owned),
        ..RawModel::default()
    };
    if let Some(textures) = value.get("textures").and_then(Value::as_object) {
        for (key, entry) in textures {
            if let Some(reference) = entry.as_str() {
                // Vanilla keys never carry the `#`, but hand-written packs
                // sometimes do; normalise so `#alias` still resolves.
                let key = key.strip_prefix('#').unwrap_or(key);
                model.textures.insert(key.to_owned(), reference.to_owned());
            }
        }
    }
    if let Some(elements) = value.get("elements").and_then(Value::as_array) {
        let parsed: Vec<RawElement> = elements.iter().filter_map(parse_element).collect();
        model.elements = Some(parsed);
    }
    model
}

fn parse_element(value: &Value) -> Option<RawElement> {
    let from = triple(value.get("from")?)?;
    let to = triple(value.get("to")?)?;
    let rotation = value.get("rotation").and_then(|rotation| {
        let axis = match rotation.get("axis").and_then(Value::as_str)? {
            "x" => 0_u8,
            "y" => 1_u8,
            "z" => 2_u8,
            _ => return None,
        };
        let angle = rotation.get("angle").and_then(Value::as_f64)? as f32;
        let origin = rotation
            .get("origin")
            .and_then(triple)
            .unwrap_or([8.0, 8.0, 8.0]);
        Some((axis, angle, origin))
    });
    let mut faces = BTreeMap::new();
    if let Some(object) = value.get("faces").and_then(Value::as_object) {
        for (name, entry) in object {
            let Some(texture) = entry.get("texture").and_then(Value::as_str) else {
                continue;
            };
            faces.insert(
                name.clone(),
                RawFace {
                    texture: texture.to_owned(),
                    uv: entry.get("uv").and_then(triple_uv),
                    cullface: entry
                        .get("cullface")
                        .and_then(Value::as_str)
                        .and_then(FaceDir::from_name),
                    rotation: entry
                        .get("rotation")
                        .and_then(Value::as_u64)
                        .map_or(0, |value| value as u8),
                    tintindex: entry
                        .get("tintindex")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                },
            );
        }
    }
    Some(RawElement {
        from,
        to,
        rotation,
        faces,
    })
}

fn triple(value: &Value) -> Option<[f32; 3]> {
    let array = value.as_array()?;
    if array.len() != 3 {
        return None;
    }
    Some([
        array[0].as_f64()? as f32,
        array[1].as_f64()? as f32,
        array[2].as_f64()? as f32,
    ])
}

fn triple_uv(value: &Value) -> Option<[f32; 4]> {
    let array = value.as_array()?;
    if array.len() != 4 {
        return None;
    }
    Some([
        array[0].as_f64()? as f32,
        array[1].as_f64()? as f32,
        array[2].as_f64()? as f32,
        array[3].as_f64()? as f32,
    ])
}

fn normalize_model_name(name: &str) -> String {
    let (namespace, path) = super::split_reference(name);
    let path = path.strip_prefix("models/").unwrap_or(path);
    let path = path.strip_suffix(".json").unwrap_or(path);
    if namespace == "minecraft" {
        path.to_owned()
    } else {
        format!("{namespace}:{path}")
    }
}

fn face_name(face: FaceDir) -> &'static str {
    match face {
        FaceDir::NegY => "down",
        FaceDir::PosY => "up",
        FaceDir::NegZ => "north",
        FaceDir::PosZ => "south",
        FaceDir::NegX => "west",
        FaceDir::PosX => "east",
    }
}

fn resolve_texture_ref(model: &RawModel, reference: &str) -> Option<String> {
    let mut current = reference;
    for _ in 0..8 {
        if let Some(alias) = current.strip_prefix('#') {
            current = model.textures.get(alias)?.as_str();
        } else {
            return Some(current.to_owned());
        }
    }
    None
}

/// Blockstate variant selection or multipart evaluation.
fn applications(blockstate: &Value, state: &BlockState) -> Option<Vec<(String, f32, f32)>> {
    if let Some(variants) = blockstate.get("variants").and_then(Value::as_object) {
        let mut best: Option<(usize, &Value)> = None;
        for (key, value) in variants {
            let Some(score) = variant_score(key, state) else {
                continue;
            };
            let better = match best {
                None => true,
                Some((best_score, _)) => score > best_score,
            };
            if better {
                best = Some((score, value));
            }
        }
        return best.map(|(_, value)| apply_values(value));
    }

    let parts = blockstate.get("multipart").and_then(Value::as_array)?;
    let mut applied = Vec::new();
    for part in parts {
        let matches = match part.get("when") {
            None => true,
            Some(when) => matches_when(when, state),
        };
        if matches {
            if let Some(apply) = part.get("apply") {
                applied.extend(apply_values(apply));
            }
        }
    }
    if applied.is_empty() {
        None
    } else {
        Some(applied)
    }
}

fn apply_values(value: &Value) -> Vec<(String, f32, f32)> {
    match value {
        Value::Object(_) => vec![apply_one(value)],
        Value::Array(items) => items.iter().map(apply_one).collect(),
        _ => Vec::new(),
    }
}

fn apply_one(value: &Value) -> (String, f32, f32) {
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let x = value.get("x").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let y = value.get("y").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    (model, x, y)
}

fn variant_score(key: &str, state: &BlockState) -> Option<usize> {
    let mut matched = 0_usize;
    for pair in key.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=')?;
        if state.property(name)? != value {
            return None;
        }
        matched += 1;
    }
    Some(matched)
}

fn matches_when(when: &Value, state: &BlockState) -> bool {
    match when {
        Value::Object(map) => {
            for (key, value) in map {
                match key.as_str() {
                    "OR" => {
                        let any = value.as_array().is_some_and(|items| {
                            items.iter().any(|item| matches_when(item, state))
                        });
                        if !any {
                            return false;
                        }
                    }
                    "AND" => {
                        let all = value.as_array().is_some_and(|items| {
                            items.iter().all(|item| matches_when(item, state))
                        });
                        if !all {
                            return false;
                        }
                    }
                    property => {
                        let expected = value.as_str().unwrap_or_default();
                        let actual = state.property(property).unwrap_or_default();
                        let hit = expected.split('|').any(|candidate| candidate == actual);
                        if !hit {
                            return false;
                        }
                    }
                }
            }
            true
        }
        Value::Array(items) => items.iter().any(|item| matches_when(item, state)),
        _ => true,
    }
}

fn block_rotation(x: f32, y: f32) -> Option<ElementTransform> {
    let center = [8.0, 8.0, 8.0];
    let rotate_x = x.abs() > f32::EPSILON;
    let rotate_y = y.abs() > f32::EPSILON;
    match (rotate_x, rotate_y) {
        (false, false) => None,
        (true, false) => Some(ElementTransform::rotation(0, x, center)),
        (false, true) => Some(ElementTransform::rotation(1, y, center)),
        // Vanilla applies X first, then Y.
        (true, true) => Some(
            ElementTransform::rotation(1, y, center).then(ElementTransform::rotation(0, x, center)),
        ),
    }
}

/// Biome tint inferred from the block name when a model sets `tintindex`.
fn tint_kind(short: &str) -> TintKind {
    if short.contains("water") {
        TintKind::Water
    } else if short.contains("leaves") {
        TintKind::Foliage
    } else if short.contains("grass") || short.contains("fern") || short.contains("vine") {
        TintKind::Grass
    } else {
        TintKind::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::pack_png;
    use std::io::Write;
    use std::path::PathBuf;

    struct TempPack {
        root: PathBuf,
    }

    impl TempPack {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rkb-pack-{name}"));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("assets/minecraft/blockstates")).unwrap();
            std::fs::create_dir_all(root.join("assets/minecraft/models/block")).unwrap();
            std::fs::create_dir_all(root.join("assets/minecraft/textures/block")).unwrap();
            std::fs::write(
                root.join("pack.mcmeta"),
                r#"{"pack":{"pack_format":34,"description":"RKB test pack"}}"#,
            )
            .unwrap();
            Self { root }
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let mut file = std::fs::File::create(path).unwrap();
            file.write_all(contents.as_bytes()).unwrap();
        }

        fn write_png(&self, relative: &str, rgba: &[u8], width: u32, height: u32) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, pack_png(rgba, width, height)).unwrap();
        }

        fn open(&self) -> ResourcePack {
            ResourcePack::open(&self.root).unwrap()
        }
    }

    #[test]
    fn resolves_a_variant_model_with_parent_textures() {
        let pack = TempPack::new("variant");
        pack.write(
            "assets/minecraft/blockstates/stone.json",
            r#"{"variants":{"":{"model":"minecraft:block/stone"}}}"#,
        );
        pack.write(
            "assets/minecraft/models/block/stone.json",
            r##"{"parent":"block/cube_all","textures":{"all":"block/stone"}}"##,
        );
        pack.write(
            "assets/minecraft/models/block/cube_all.json",
            r##"{"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{
                "north":{"texture":"#all"},"south":{"texture":"#all"},
                "east":{"texture":"#all"},"west":{"texture":"#all"},
                "up":{"texture":"#all"},"down":{"texture":"#all"}}}]}"##,
        );
        pack.write_png(
            "assets/minecraft/textures/block/stone.png",
            &[10, 20, 30, 255],
            1,
            1,
        );

        let models = PackModels::new(pack.open());
        let resolved = models.resolve(&BlockState::new("minecraft:stone"));
        let crate::model::BlockShape::Elements(elements) = resolved.shape else {
            panic!("stone must resolve to elements");
        };
        assert_eq!(elements.len(), 1);
        assert_eq!(resolved.layer, RenderLayer::Opaque);
        assert!(!models.approximate(&BlockState::new("minecraft:stone")));
        assert_eq!(models.missing_textures(), 0);
        assert_eq!(models.texture_count(), 2); // missing tile + stone
        match elements[0].faces[FaceDir::NegZ.index()].unwrap().texture {
            FaceTexture::Atlas(id) => assert!(id > 0),
            other => panic!("expected atlas texture, got {other:?}"),
        }
        assert!(models.texture_count() > 1);
    }

    #[test]
    fn missing_blockstate_falls_back_and_is_counted() {
        let pack = TempPack::new("missing");
        let models = PackModels::new(pack.open());
        let state = BlockState::new("minecraft:oak_fence");
        let resolved = models.resolve(&state);
        assert!(matches!(
            resolved.shape,
            crate::model::BlockShape::Elements(_)
        ));
        assert!(models.approximate(&state));
        assert_eq!(models.unmodelled_blocks(), 1);
        // A second lookup uses the cache and does not double count.
        let _ = models.resolve(&state);
        assert_eq!(models.unmodelled_blocks(), 1);
    }

    #[test]
    fn multipart_conditions_and_rotations_apply() {
        let pack = TempPack::new("multipart");
        pack.write(
            "assets/minecraft/blockstates/stone_stairs.json",
            r#"{"multipart":[
                {"when":{"facing":"north"},"apply":{"model":"block/step","x":90}},
                {"apply":{"model":"block/step"}}]}"#,
        );
        pack.write(
            "assets/minecraft/models/block/step.json",
            r#"{"elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{
                "up":{"texture":"block/stone","uv":[0,0,16,16]}}}]}"#,
        );
        pack.write_png(
            "assets/minecraft/textures/block/stone.png",
            &[1, 2, 3, 255],
            1,
            1,
        );

        let models = PackModels::new(pack.open());
        let north = models.resolve(
            &BlockState::new("minecraft:stone_stairs")
                .with_property("facing", "north")
                .with_property("half", "bottom")
                .with_property("shape", "straight"),
        );
        let crate::model::BlockShape::Elements(elements) = north.shape else {
            panic!("stairs must resolve to elements");
        };
        assert!(
            elements[0].transform.is_some(),
            "x:90 must rotate the model"
        );

        let south = models.resolve(
            &BlockState::new("minecraft:stone_stairs")
                .with_property("facing", "south")
                .with_property("half", "bottom")
                .with_property("shape", "straight"),
        );
        let crate::model::BlockShape::Elements(elements) = south.shape else {
            panic!("stairs must resolve to elements");
        };
        assert!(
            elements[0].transform.is_none(),
            "unconditional part is plain"
        );
    }

    #[test]
    fn unknown_texture_alias_uses_the_missing_tile() {
        let pack = TempPack::new("cycle");
        pack.write(
            "assets/minecraft/blockstates/bad.json",
            r#"{"variants":{"":{"model":"block/bad"}}}"#,
        );
        pack.write(
            "assets/minecraft/models/block/bad.json",
            r##"{"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{
                "up":{"texture":"#missing"}}}]}"##,
        );
        let models = PackModels::new(pack.open());
        let state = BlockState::new("minecraft:bad");
        let resolved = models.resolve(&state);
        let crate::model::BlockShape::Elements(elements) = resolved.shape else {
            panic!("model exists so it must resolve");
        };
        assert_eq!(elements.len(), 1);
        let face = elements[0].faces[FaceDir::PosY.index()].expect("up face");
        match face.texture {
            FaceTexture::Atlas(id) => assert_eq!(id, models.store().missing_id()),
            other => panic!("expected atlas texture, got {other:?}"),
        }
        assert_eq!(models.missing_textures(), 1);
    }
}
