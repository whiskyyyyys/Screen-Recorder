use crate::model::{
    BoundsType, CropRect, ItemTransform, ResolvedRenderItem, Scene, SceneItem, SourceDefinition,
    SourceKind, SourceProperties,
};
use obs_core::{ObsError, Result, SceneId, SceneItemId, SourceId, TransitionId};
use obs_transitions::TransitionConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCollection {
    pub name: String,
    pub scenes: Vec<Scene>,
    pub sources: HashMap<SourceId, SourceDefinition>,
    pub transitions: Vec<TransitionConfig>,
    pub active_scene_id: SceneId,
    pub preview_scene_id: Option<SceneId>,
    pub active_transition_id: TransitionId,
    pub transition_duration_ms: u32,
}

impl SceneCollection {
    pub fn default_starter(name: impl Into<String>) -> Self {
        let mut sources = HashMap::new();

        let bg_id = SourceId::new();
        sources.insert(
            bg_id,
            SourceDefinition {
                id: bg_id,
                name: "Display Capture".to_string(),
                kind: SourceKind::DisplayCapture,
                properties: SourceProperties::default_for(SourceKind::DisplayCapture),
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );

        let desktop_audio_id = SourceId::new();
        sources.insert(
            desktop_audio_id,
            SourceDefinition {
                id: desktop_audio_id,
                name: "Desktop Audio".to_string(),
                kind: SourceKind::AudioOutputCapture,
                properties: SourceProperties::default_for(SourceKind::AudioOutputCapture),
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );

        let mic_id = SourceId::new();
        sources.insert(
            mic_id,
            SourceDefinition {
                id: mic_id,
                name: "Mic/Aux".to_string(),
                kind: SourceKind::AudioInputCapture,
                properties: SourceProperties::default_for(SourceKind::AudioInputCapture),
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );

        let scene1_id = SceneId::new();
        let scene2_id = SceneId::new();

        let badge_id = SourceId::new();
        sources.insert(
            badge_id,
            SourceDefinition {
                id: badge_id,
                name: "BRB Banner Color".to_string(),
                kind: SourceKind::ColorSource,
                properties: SourceProperties::ColorSource {
                    color_rgba: [18, 24, 38, 255],
                    width: 1920,
                    height: 1080,
                },
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );

        let brb_text_id = SourceId::new();
        sources.insert(
            brb_text_id,
            SourceDefinition {
                id: brb_text_id,
                name: "BRB Title".to_string(),
                kind: SourceKind::TextSource,
                properties: SourceProperties::TextSource {
                    text: "Starting Soon — OxideStudio".to_string(),
                    font_family: "Segoe UI".to_string(),
                    font_size_px: 72.0,
                    color_rgba: [245, 248, 255, 255],
                    gradient_enabled: false,
                    gradient_bottom_rgba: [150, 200, 255, 255],
                    outline_enabled: true,
                    outline_size: 3.0,
                    outline_color_rgba: [0, 0, 0, 255],
                    drop_shadow: true,
                    align: "center".to_string(),
                    read_from_file: false,
                    file_path: String::new(),
                },
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );

        let mut text_item = SceneItem::new(brb_text_id);
        text_item.transform.pos = [420.0, 480.0];

        let scenes = vec![
            Scene {
                id: scene1_id,
                name: "Main Scene".to_string(),
                items: vec![
                    SceneItem::new(bg_id),
                    SceneItem::new(desktop_audio_id),
                    SceneItem::new(mic_id),
                ],
            },
            Scene {
                id: scene2_id,
                name: "Starting Soon".to_string(),
                items: vec![SceneItem::new(badge_id), text_item],
            },
        ];

        let transitions = TransitionConfig::default_presets();
        let active_transition_id = transitions[1].id; // Fade

        Self {
            name: name.into(),
            scenes,
            sources,
            transitions,
            active_scene_id: scene1_id,
            preview_scene_id: Some(scene2_id),
            active_transition_id,
            transition_duration_ms: 300,
        }
    }

    pub fn active_scene(&self) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id == self.active_scene_id)
    }

    pub fn active_scene_mut(&mut self) -> Option<&mut Scene> {
        let id = self.active_scene_id;
        self.scenes.iter_mut().find(|s| s.id == id)
    }

    pub fn scene_by_id(&self, id: SceneId) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id == id)
    }

    pub fn scene_by_id_mut(&mut self, id: SceneId) -> Option<&mut Scene> {
        self.scenes.iter_mut().find(|s| s.id == id)
    }

    pub fn scene_by_name(&self, name: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.name == name)
    }

    pub fn active_transition(&self) -> Option<&TransitionConfig> {
        self.transitions
            .iter()
            .find(|t| t.id == self.active_transition_id)
            .or_else(|| self.transitions.first())
    }

    pub fn create_scene(&mut self, name: impl Into<String>) -> Result<SceneId> {
        let requested = name.into().trim().to_string();
        if requested.is_empty() {
            return Err(ObsError::InvalidArgument(
                "Scene name cannot be empty".to_string(),
            ));
        }
        let final_name = self.unique_scene_name(&requested);
        let id = SceneId::new();
        self.scenes.push(Scene {
            id,
            name: final_name,
            items: Vec::new(),
        });
        Ok(id)
    }

    pub fn rename_scene(&mut self, scene_id: SceneId, new_name: impl Into<String>) -> Result<()> {
        let trimmed = new_name.into().trim().to_string();
        if trimmed.is_empty() {
            return Err(ObsError::InvalidArgument(
                "Scene name cannot be empty".to_string(),
            ));
        }
        if self
            .scenes
            .iter()
            .any(|s| s.id != scene_id && s.name == trimmed)
        {
            return Err(ObsError::AlreadyExists(format!("Scene '{trimmed}'")));
        }
        let scene = self
            .scene_by_id_mut(scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        scene.name = trimmed;
        Ok(())
    }

    pub fn duplicate_scene(
        &mut self,
        scene_id: SceneId,
        new_name: Option<String>,
    ) -> Result<SceneId> {
        let src = self
            .scene_by_id(scene_id)
            .cloned()
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        let base_name = new_name.unwrap_or_else(|| format!("{} Copy", src.name));
        let final_name = self.unique_scene_name(&base_name);
        let new_id = SceneId::new();
        let duplicated_items = src
            .items
            .into_iter()
            .map(clone_scene_item_new_ids)
            .collect();
        self.scenes.push(Scene {
            id: new_id,
            name: final_name,
            items: duplicated_items,
        });
        Ok(new_id)
    }

    pub fn delete_scene(&mut self, scene_id: SceneId) -> Result<()> {
        if self.scenes.len() <= 1 {
            return Err(ObsError::InvalidArgument(
                "Cannot delete the last remaining scene".to_string(),
            ));
        }
        let idx = self
            .scenes
            .iter()
            .position(|s| s.id == scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        self.scenes.remove(idx);
        if self.active_scene_id == scene_id {
            self.active_scene_id = self.scenes[0].id;
        }
        if self.preview_scene_id == Some(scene_id) {
            self.preview_scene_id = Some(self.scenes[0].id);
        }
        Ok(())
    }

    pub fn reorder_scene(&mut self, from_idx: usize, to_idx: usize) -> Result<()> {
        if from_idx >= self.scenes.len() || to_idx >= self.scenes.len() {
            return Err(ObsError::InvalidArgument("Invalid scene index".to_string()));
        }
        let scene = self.scenes.remove(from_idx);
        self.scenes.insert(to_idx, scene);
        Ok(())
    }

    pub fn create_source_and_add(
        &mut self,
        scene_id: SceneId,
        name: impl Into<String>,
        kind: SourceKind,
        properties: SourceProperties,
    ) -> Result<(SourceId, SceneItemId)> {
        if let SourceProperties::SceneSource {
            scene_id: target_scene_id,
        } = &properties
        {
            if self.would_scene_reference_create_cycle(scene_id, *target_scene_id) {
                return Err(ObsError::SceneCycle(
                    "Adding this nested scene would create a cycle".to_string(),
                ));
            }
        }

        let requested = name.into().trim().to_string();
        let final_name = self.unique_source_name(if requested.is_empty() {
            kind.display_name()
        } else {
            &requested
        });
        let source_id = SourceId::new();
        self.sources.insert(
            source_id,
            SourceDefinition {
                id: source_id,
                name: final_name,
                kind,
                properties,
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );
        let item_id = self.add_existing_source_to_scene(scene_id, source_id)?;
        Ok((source_id, item_id))
    }

    /// Reference-clones an existing shared source into `scene_id` without duplicating the underlying source.
    pub fn add_existing_source_to_scene(
        &mut self,
        scene_id: SceneId,
        source_id: SourceId,
    ) -> Result<SceneItemId> {
        let source = self
            .sources
            .get(&source_id)
            .ok_or_else(|| ObsError::NotFound("Source not found".to_string()))?;
        if let SourceProperties::SceneSource {
            scene_id: target_scene_id,
        } = &source.properties
        {
            if self.would_scene_reference_create_cycle(scene_id, *target_scene_id) {
                return Err(ObsError::SceneCycle(
                    "Nested scene reference creates a cycle".to_string(),
                ));
            }
        }
        let item = SceneItem::new(source_id);
        let item_id = item.id;
        let scene = self
            .scene_by_id_mut(scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        scene.items.push(item);
        Ok(item_id)
    }

    pub fn remove_scene_item(&mut self, scene_id: SceneId, item_id: SceneItemId) -> Result<()> {
        let scene = self
            .scene_by_id_mut(scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        if !remove_item_recursive(&mut scene.items, item_id) {
            return Err(ObsError::NotFound("Scene item not found".to_string()));
        }
        Ok(())
    }

    pub fn remove_source_globally(&mut self, source_id: SourceId) -> Result<()> {
        if self.sources.remove(&source_id).is_none() {
            return Err(ObsError::NotFound("Source not found".to_string()));
        }
        for scene in &mut self.scenes {
            remove_source_refs_recursive(&mut scene.items, source_id);
        }
        Ok(())
    }

    pub fn group_items(
        &mut self,
        scene_id: SceneId,
        item_ids: &[SceneItemId],
        group_name: impl Into<String>,
    ) -> Result<SceneItemId> {
        if item_ids.is_empty() {
            return Err(ObsError::InvalidArgument(
                "Select at least one item to group".to_string(),
            ));
        }
        let group_source_id = SourceId::new();
        let final_name = self.unique_source_name(&group_name.into());
        self.sources.insert(
            group_source_id,
            SourceDefinition {
                id: group_source_id,
                name: final_name,
                kind: SourceKind::GroupSource,
                properties: SourceProperties::GroupSource,
                video_filters: Vec::new(),
                audio_filters: Vec::new(),
                audio: Default::default(),
            },
        );
        let scene = self
            .scene_by_id_mut(scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;

        let set: HashSet<SceneItemId> = item_ids.iter().copied().collect();
        let mut children = Vec::new();
        let mut insert_at = None;
        let mut remaining = Vec::with_capacity(scene.items.len());
        for (idx, item) in scene.items.drain(..).enumerate() {
            if set.contains(&item.id) {
                if insert_at.is_none() {
                    insert_at = Some(idx);
                }
                children.push(item);
            } else {
                remaining.push(item);
            }
        }
        if children.is_empty() {
            scene.items = remaining;
            self.sources.remove(&group_source_id);
            return Err(ObsError::NotFound("No matching items in scene".to_string()));
        }
        let group_item = SceneItem {
            id: SceneItemId::new(),
            source_id: group_source_id,
            visible: true,
            locked: false,
            blend_mode: Default::default(),
            transform: ItemTransform::default(),
            group_children: Some(children),
            group_expanded: true,
        };
        let gid = group_item.id;
        let pos = insert_at.unwrap_or(remaining.len()).min(remaining.len());
        remaining.insert(pos, group_item);
        scene.items = remaining;
        Ok(gid)
    }

    pub fn ungroup_item(&mut self, scene_id: SceneId, group_item_id: SceneItemId) -> Result<()> {
        let scene = self
            .scene_by_id_mut(scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        let idx = scene
            .items
            .iter()
            .position(|i| i.id == group_item_id && i.is_group())
            .ok_or_else(|| ObsError::NotFound("Group item not found".to_string()))?;
        let group = scene.items.remove(idx);
        let group_source_id = group.source_id;
        let group_pos = group.transform.pos;
        if let Some(children) = group.group_children {
            for (offset, mut child) in children.into_iter().enumerate() {
                child.transform.pos[0] += group_pos[0];
                child.transform.pos[1] += group_pos[1];
                scene.items.insert(idx + offset, child);
            }
        }
        self.sources.remove(&group_source_id);
        Ok(())
    }

    pub fn find_item(&self, scene_id: SceneId, item_id: SceneItemId) -> Option<&SceneItem> {
        let scene = self.scene_by_id(scene_id)?;
        find_item_recursive(&scene.items, item_id)
    }

    pub fn find_item_mut(
        &mut self,
        scene_id: SceneId,
        item_id: SceneItemId,
    ) -> Option<&mut SceneItem> {
        let scene = self.scene_by_id_mut(scene_id)?;
        find_item_recursive_mut(&mut scene.items, item_id)
    }

    pub fn move_item_step(
        &mut self,
        scene_id: SceneId,
        item_id: SceneItemId,
        delta: isize,
    ) -> Result<()> {
        let scene = self
            .scene_by_id_mut(scene_id)
            .ok_or_else(|| ObsError::NotFound("Scene not found".to_string()))?;
        let idx = scene
            .items
            .iter()
            .position(|i| i.id == item_id)
            .ok_or_else(|| ObsError::NotFound("Scene item not found".to_string()))?;
        let target = (idx as isize + delta).clamp(0, (scene.items.len() - 1) as isize) as usize;
        if target != idx {
            let item = scene.items.remove(idx);
            scene.items.insert(target, item);
        }
        Ok(())
    }

    /// Cycle detection: checks if adding `child_scene_id` inside `parent_scene_id` reaches `parent_scene_id`.
    pub fn would_scene_reference_create_cycle(
        &self,
        parent_scene_id: SceneId,
        child_scene_id: SceneId,
    ) -> bool {
        if parent_scene_id == child_scene_id {
            return true;
        }
        let mut stack = vec![child_scene_id];
        let mut visited = HashSet::new();
        while let Some(curr) = stack.pop() {
            if curr == parent_scene_id {
                return true;
            }
            if !visited.insert(curr) {
                continue;
            }
            if let Some(scene) = self.scene_by_id(curr) {
                self.collect_referenced_scenes(&scene.items, &mut stack);
            }
        }
        false
    }

    fn collect_referenced_scenes(&self, items: &[SceneItem], out: &mut Vec<SceneId>) {
        for item in items {
            if let Some(src) = self.sources.get(&item.source_id) {
                if let SourceProperties::SceneSource { scene_id } = src.properties {
                    out.push(scene_id);
                }
            }
            if let Some(children) = &item.group_children {
                self.collect_referenced_scenes(children, out);
            }
        }
    }

    /// Resolves visible scene items, expanding Groups and Nested Scenes into flat render commands.
    pub fn resolve_render_items(&self, scene_id: SceneId) -> Vec<ResolvedRenderItem> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        self.resolve_scene_recursive(
            scene_id,
            [0.0, 0.0],
            [1.0, 1.0],
            0.0,
            1.0,
            &mut visited,
            &mut out,
        );
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_scene_recursive(
        &self,
        scene_id: SceneId,
        parent_pos: [f32; 2],
        parent_scale: [f32; 2],
        parent_rot: f32,
        parent_opacity: f32,
        visited: &mut HashSet<SceneId>,
        out: &mut Vec<ResolvedRenderItem>,
    ) {
        if !visited.insert(scene_id) {
            return;
        }
        if let Some(scene) = self.scene_by_id(scene_id) {
            self.resolve_items_slice(
                &scene.items,
                parent_pos,
                parent_scale,
                parent_rot,
                parent_opacity,
                visited,
                out,
            );
        }
        visited.remove(&scene_id);
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_items_slice(
        &self,
        items: &[SceneItem],
        parent_pos: [f32; 2],
        parent_scale: [f32; 2],
        parent_rot: f32,
        parent_opacity: f32,
        visited: &mut HashSet<SceneId>,
        out: &mut Vec<ResolvedRenderItem>,
    ) {
        for item in items {
            if !item.visible {
                continue;
            }
            let pos = [
                parent_pos[0] + item.transform.pos[0] * parent_scale[0],
                parent_pos[1] + item.transform.pos[1] * parent_scale[1],
            ];
            let scale = [
                parent_scale[0] * item.transform.scale[0],
                parent_scale[1] * item.transform.scale[1],
            ];
            let rot = parent_rot + item.transform.rotation_deg;
            let opacity = (parent_opacity * item.transform.opacity).clamp(0.0, 1.0);

            if let Some(children) = &item.group_children {
                self.resolve_items_slice(children, pos, scale, rot, opacity, visited, out);
                continue;
            }

            let Some(src) = self.sources.get(&item.source_id) else {
                continue;
            };
            if let SourceProperties::SceneSource { scene_id: nested } = src.properties {
                self.resolve_scene_recursive(nested, pos, scale, rot, opacity, visited, out);
                continue;
            }
            if !src.kind.has_video() {
                continue;
            }
            out.push(ResolvedRenderItem {
                item_id: item.id,
                source_id: item.source_id,
                pos,
                scale,
                rotation_deg: rot,
                crop: item.transform.crop,
                bounds_type: item.transform.bounds_type,
                bounds_size: item.transform.bounds_size,
                opacity,
                blend_mode: item.blend_mode,
                filters: src.video_filters.clone(),
            });
        }
    }

    pub fn fit_item_to_canvas(
        transform: &mut ItemTransform,
        source_size: [f32; 2],
        canvas_size: [f32; 2],
    ) {
        let sw = source_size[0].max(1.0);
        let sh = source_size[1].max(1.0);
        let ratio = (canvas_size[0] / sw).min(canvas_size[1] / sh);
        transform.scale = [ratio, ratio];
        transform.pos = [
            (canvas_size[0] - sw * ratio) * 0.5,
            (canvas_size[1] - sh * ratio) * 0.5,
        ];
        transform.rotation_deg = 0.0;
        transform.bounds_type = BoundsType::None;
    }

    pub fn stretch_item_to_canvas(
        transform: &mut ItemTransform,
        source_size: [f32; 2],
        canvas_size: [f32; 2],
    ) {
        let sw = source_size[0].max(1.0);
        let sh = source_size[1].max(1.0);
        transform.scale = [canvas_size[0] / sw, canvas_size[1] / sh];
        transform.pos = [0.0, 0.0];
        transform.rotation_deg = 0.0;
        transform.crop = CropRect::default();
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load_from_file(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)?;
        let col = serde_json::from_str::<Self>(&raw)?;
        Ok(col)
    }

    fn unique_scene_name(&self, base: &str) -> String {
        if !self.scenes.iter().any(|s| s.name == base) {
            return base.to_string();
        }
        let mut idx = 2;
        loop {
            let candidate = format!("{base} ({idx})");
            if !self.scenes.iter().any(|s| s.name == candidate) {
                return candidate;
            }
            idx += 1;
        }
    }

    fn unique_source_name(&self, base: &str) -> String {
        if !self.sources.values().any(|s| s.name == base) {
            return base.to_string();
        }
        let mut idx = 2;
        loop {
            let candidate = format!("{base} ({idx})");
            if !self.sources.values().any(|s| s.name == candidate) {
                return candidate;
            }
            idx += 1;
        }
    }
}

fn clone_scene_item_new_ids(mut item: SceneItem) -> SceneItem {
    item.id = SceneItemId::new();
    if let Some(children) = item.group_children {
        item.group_children = Some(children.into_iter().map(clone_scene_item_new_ids).collect());
    }
    item
}

fn remove_item_recursive(items: &mut Vec<SceneItem>, item_id: SceneItemId) -> bool {
    if let Some(pos) = items.iter().position(|i| i.id == item_id) {
        items.remove(pos);
        return true;
    }
    for item in items.iter_mut() {
        if let Some(children) = &mut item.group_children {
            if remove_item_recursive(children, item_id) {
                return true;
            }
        }
    }
    false
}

fn remove_source_refs_recursive(items: &mut Vec<SceneItem>, source_id: SourceId) {
    items.retain(|i| i.source_id != source_id);
    for item in items.iter_mut() {
        if let Some(children) = &mut item.group_children {
            remove_source_refs_recursive(children, source_id);
        }
    }
}

fn find_item_recursive(items: &[SceneItem], item_id: SceneItemId) -> Option<&SceneItem> {
    for item in items {
        if item.id == item_id {
            return Some(item);
        }
        if let Some(children) = &item.group_children {
            if let Some(found) = find_item_recursive(children, item_id) {
                return Some(found);
            }
        }
    }
    None
}

fn find_item_recursive_mut(
    items: &mut [SceneItem],
    item_id: SceneItemId,
) -> Option<&mut SceneItem> {
    for item in items {
        if item.id == item_id {
            return Some(item);
        }
        if let Some(children) = &mut item.group_children {
            if let Some(found) = find_item_recursive_mut(children, item_id) {
                return Some(found);
            }
        }
    }
    None
}
