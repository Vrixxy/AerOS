//! The single integration point for any UI.
//!
//! UI loop: `state.apply(Command::..)` on input, `state.render(size)` to get a
//! frame, upload the RGBA bytes as a texture. The state owns undo/redo.

use crate::model::*;
use crate::render::{render, to_rgba8};
use crate::rig::{Expression, Pose};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub enum Command {
    SetName(String),
    SetSkin(Rgb),
    SetHairColor(Rgb),
    SetEyeColor(Rgb),
    SetLipColor(Rgb),
    SetOutfitColor(Rgb),
    SetHead(HeadShape),
    SetHair(HairStyle),
    SetEyes(EyeStyle),
    SetBrows(BrowStyle),
    SetNose(NoseStyle),
    SetMouth(MouthStyle),
    SetFacialHair(FacialHair),
    SetGlasses(Glasses),
    SetOutfit(Outfit),
    Randomize(u64),
    Reset,
    /// Live, not undoable.
    SetExpression(Expression),
    SetPose(Pose),
    Undo,
    Redo,
    /// Replace the avatar from `Avatar::to_text` output.
    LoadText(String),
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct AppState {
    pub avatar: Avatar,
    pub expression: Expression,
    pub pose: Pose,
    pub background: Option<Rgb>,
    undo: Vec<Avatar>,
    redo: Vec<Avatar>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn avatar_text(&self) -> String {
        self.avatar.to_text()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn edit(&mut self, f: impl FnOnce(&mut Avatar)) {
        let before = self.avatar.clone();
        f(&mut self.avatar);
        if self.avatar != before {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    pub fn apply(&mut self, cmd: Command) -> crate::Result<()> {
        use Command::*;
        match cmd {
            SetName(v) => self.edit(|a| a.name = v),
            SetSkin(v) => self.edit(|a| a.skin = v),
            SetHairColor(v) => self.edit(|a| a.hair_color = v),
            SetEyeColor(v) => self.edit(|a| a.eye_color = v),
            SetLipColor(v) => self.edit(|a| a.lip_color = v),
            SetOutfitColor(v) => self.edit(|a| a.outfit_color = v),
            SetHead(v) => self.edit(|a| a.head = v),
            SetHair(v) => self.edit(|a| a.hair = v),
            SetEyes(v) => self.edit(|a| a.eyes = v),
            SetBrows(v) => self.edit(|a| a.brows = v),
            SetNose(v) => self.edit(|a| a.nose = v),
            SetMouth(v) => self.edit(|a| a.mouth = v),
            SetFacialHair(v) => self.edit(|a| a.facial_hair = v),
            SetGlasses(v) => self.edit(|a| a.glasses = v),
            SetOutfit(v) => self.edit(|a| a.outfit = v),
            Randomize(seed) => self.edit(|a| *a = Avatar::randomized(seed)),
            Reset => self.edit(|a| *a = Avatar::default()),
            SetExpression(e) => self.expression = e,
            SetPose(p) => self.pose = p,
            Undo => {
                if let Some(prev) = self.undo.pop() {
                    self.redo.push(core::mem::replace(&mut self.avatar, prev));
                }
            }
            Redo => {
                if let Some(next) = self.redo.pop() {
                    self.undo.push(core::mem::replace(&mut self.avatar, next));
                }
            }
            LoadText(t) => {
                let loaded = Avatar::from_text(&t)?;
                self.edit(|a| *a = loaded);
            }
        }
        Ok(())
    }

    pub fn render(&self, size: u32) -> Frame {
        let sf = render(
            &self.avatar,
            &self.expression,
            &self.pose,
            size,
            self.background,
        );
        Frame {
            width: sf.w as u32,
            height: sf.h as u32,
            rgba: to_rgba8(&sf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_roundtrip() {
        let mut s = AppState::new();
        s.apply(Command::SetHair(HairStyle::Curly)).unwrap();
        assert_eq!(s.avatar.hair, HairStyle::Curly);
        s.apply(Command::Undo).unwrap();
        assert_eq!(s.avatar.hair, HairStyle::Short);
        s.apply(Command::Redo).unwrap();
        assert_eq!(s.avatar.hair, HairStyle::Curly);
    }

    #[test]
    fn frame_is_rgba() {
        let f = AppState::new().render(64);
        assert_eq!(f.rgba.len(), (64 * 64 * 4) as usize);
    }
}
