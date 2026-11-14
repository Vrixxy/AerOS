use crate::error::{Error, Result};
use alloc::format;
use alloc::string::{String, ToString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn darken(self, f: f32) -> Rgb {
        let m = |c: u8| (c as f32 * (1.0 - f)).clamp(0.0, 255.0) as u8;
        Rgb(m(self.0), m(self.1), m(self.2))
    }
    pub fn lighten(self, f: f32) -> Rgb {
        let m = |c: u8| (c as f32 + (255.0 - c as f32) * f).clamp(0.0, 255.0) as u8;
        Rgb(m(self.0), m(self.1), m(self.2))
    }
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let p = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        Some(Rgb(p(0)?, p(2)?, p(4)?))
    }
}

/// Declares a fieldless enum with `ALL`, `label()` and `from_label()` so a UI can list options.
macro_rules! variants {
    ($name:ident { $($v:ident),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($v),* }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),*];
            pub fn label(self) -> &'static str {
                match self { $($name::$v => stringify!($v)),* }
            }
            pub fn from_label(s: &str) -> Option<$name> {
                match s { $(stringify!($v) => Some($name::$v),)* _ => None }
            }
        }
    };
}

variants!(HeadShape {
    Round,
    Oval,
    Square,
    Heart
});
variants!(HairStyle {
    Bald,
    Buzz,
    Short,
    Long,
    Curly,
    Bun
});
variants!(EyeStyle {
    Round,
    Almond,
    Wide,
    Sleepy
});
variants!(BrowStyle {
    Soft,
    Straight,
    Thick,
    Arched
});
variants!(NoseStyle {
    Button,
    Pointed,
    Wide
});
variants!(MouthStyle {
    Thin,
    Regular,
    Full
});
variants!(FacialHair {
    None,
    Stubble,
    Moustache,
    Beard
});
variants!(Glasses {
    None,
    Round,
    Square
});
variants!(Outfit {
    Tee,
    Hoodie,
    Collar
});

/// A persisted avatar: only appearance choices, no expression or pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Avatar {
    pub name: String,
    pub skin: Rgb,
    pub hair_color: Rgb,
    pub eye_color: Rgb,
    pub lip_color: Rgb,
    pub outfit_color: Rgb,
    pub head: HeadShape,
    pub hair: HairStyle,
    pub eyes: EyeStyle,
    pub brows: BrowStyle,
    pub nose: NoseStyle,
    pub mouth: MouthStyle,
    pub facial_hair: FacialHair,
    pub glasses: Glasses,
    pub outfit: Outfit,
}

impl Default for Avatar {
    fn default() -> Self {
        Avatar {
            name: "Me".to_string(),
            skin: Rgb(240, 194, 160),
            hair_color: Rgb(58, 38, 28),
            eye_color: Rgb(70, 110, 160),
            lip_color: Rgb(196, 110, 108),
            outfit_color: Rgb(90, 130, 220),
            head: HeadShape::Round,
            hair: HairStyle::Short,
            eyes: EyeStyle::Round,
            brows: BrowStyle::Soft,
            nose: NoseStyle::Button,
            mouth: MouthStyle::Regular,
            facial_hair: FacialHair::None,
            glasses: Glasses::None,
            outfit: Outfit::Tee,
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

impl Avatar {
    pub fn randomized(seed: u64) -> Avatar {
        use crate::catalog::*;
        let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        Avatar {
            name: "Random".to_string(),
            skin: r.pick(SKIN_TONES),
            hair_color: r.pick(HAIR_COLORS),
            eye_color: r.pick(EYE_COLORS),
            lip_color: r.pick(LIP_COLORS),
            outfit_color: r.pick(OUTFIT_COLORS),
            head: r.pick(HeadShape::ALL),
            hair: r.pick(HairStyle::ALL),
            eyes: r.pick(EyeStyle::ALL),
            brows: r.pick(BrowStyle::ALL),
            nose: r.pick(NoseStyle::ALL),
            mouth: r.pick(MouthStyle::ALL),
            facial_hair: r.pick(FacialHair::ALL),
            glasses: r.pick(Glasses::ALL),
            outfit: r.pick(Outfit::ALL),
        }
    }

    /// `key=value` lines; stable and easy to store in any filesystem.
    pub fn to_text(&self) -> String {
        format!(
            "name={}\nskin={}\nhair_color={}\neye_color={}\nlip_color={}\noutfit_color={}\nhead={}\nhair={}\neyes={}\nbrows={}\nnose={}\nmouth={}\nfacial_hair={}\nglasses={}\noutfit={}\n",
            self.name,
            self.skin.hex(),
            self.hair_color.hex(),
            self.eye_color.hex(),
            self.lip_color.hex(),
            self.outfit_color.hex(),
            self.head.label(),
            self.hair.label(),
            self.eyes.label(),
            self.brows.label(),
            self.nose.label(),
            self.mouth.label(),
            self.facial_hair.label(),
            self.glasses.label(),
            self.outfit.label(),
        )
    }

    /// Unknown keys are ignored and missing keys keep their defaults.
    pub fn from_text(text: &str) -> Result<Avatar> {
        let mut a = Avatar::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let bad = || Error::Parse(format!("bad value for {k}: {v}"));
            match k {
                "name" => a.name = v.to_string(),
                "skin" => a.skin = Rgb::from_hex(v).ok_or_else(bad)?,
                "hair_color" => a.hair_color = Rgb::from_hex(v).ok_or_else(bad)?,
                "eye_color" => a.eye_color = Rgb::from_hex(v).ok_or_else(bad)?,
                "lip_color" => a.lip_color = Rgb::from_hex(v).ok_or_else(bad)?,
                "outfit_color" => a.outfit_color = Rgb::from_hex(v).ok_or_else(bad)?,
                "head" => a.head = HeadShape::from_label(v).ok_or_else(bad)?,
                "hair" => a.hair = HairStyle::from_label(v).ok_or_else(bad)?,
                "eyes" => a.eyes = EyeStyle::from_label(v).ok_or_else(bad)?,
                "brows" => a.brows = BrowStyle::from_label(v).ok_or_else(bad)?,
                "nose" => a.nose = NoseStyle::from_label(v).ok_or_else(bad)?,
                "mouth" => a.mouth = MouthStyle::from_label(v).ok_or_else(bad)?,
                "facial_hair" => a.facial_hair = FacialHair::from_label(v).ok_or_else(bad)?,
                "glasses" => a.glasses = Glasses::from_label(v).ok_or_else(bad)?,
                "outfit" => a.outfit = Outfit::from_label(v).ok_or_else(bad)?,
                _ => {}
            }
        }
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roundtrip() {
        let a = Avatar::randomized(42);
        assert_eq!(Avatar::from_text(&a.to_text()).unwrap(), a);
    }
}
