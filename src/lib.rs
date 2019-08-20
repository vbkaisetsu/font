#![feature(thread_local, const_vec_new)]
#[macro_use] extern crate log;
#[macro_use] extern crate slotmap;

use std::fmt;
use std::convert::TryInto;
use nom::{IResult, Err::*, error::VerboseError};
use tuple::{TupleElements};
use encoding::Encoding;
use vector::{Outline, Vector, PathBuilder, Transform, Surface, Rect, PathStyle};

#[derive(Clone)]
pub struct Glyph<O: Outline> {
    /// unit 1em
    pub metrics: HMetrics,
    
    /// transform by font_matrix to scale it to 1em
    pub path: O 
}

#[derive(Copy, Clone)]
pub struct VMetrics {
    pub line_gap: f32
}
#[derive(Copy, Clone)]
pub struct HMetrics {
    pub lsb: Vector,
    pub advance: Vector
}
pub trait Font<O: Outline> {
    fn num_glyphs(&self) -> u32;
    fn font_matrix(&self) -> Transform;
    fn glyph(&self, gid: u32) -> Option<Glyph<O>>;
    fn glyphs(&self) -> Glyphs<O> {
        Glyphs {
            glyphs: (0 .. self.num_glyphs()).map(|i| self.glyph(i).unwrap()).collect()
        }
    }
    fn gid_for_codepoint(&self, _codepoint: u32) -> Option<u32> {
        None
    }
    fn gid_for_name(&self, _name: &str) -> Option<u32> {
        None
    }
    fn gid_for_unicode_codepoint(&self, codepoint: u32) -> Option<u32> {
        self.encoding()
            .and_then(|encoding| encoding.reverse_map())
            .and_then(|reverse| reverse.get(codepoint))
            .and_then(|cp| self.gid_for_codepoint(cp as u32))
    }
    fn encoding(&self) -> Option<Encoding> {
        None
    }
    fn get_notdef_gid(&self) -> u32 {
        0
    }
    fn bbox(&self) -> Option<Rect> {
        None
    }
    fn vmetrics(&self) -> Option<VMetrics> {
        None
    }
    fn kerning(&self, left: u32, right: u32) -> f32 {
        0.0
    }
}
pub struct Glyphs<O: Outline> {
    glyphs: Vec<Glyph<O>>
}
impl<O: Outline> Glyphs<O> {
    pub fn get(&self, codepoint: u32) -> Option<&Glyph<O>> {
        self.glyphs.get(codepoint as usize)
    }
}

pub fn draw_text<S: Surface>(font: &dyn Font<S::Outline>, font_size: f32, text: &str, style: PathStyle, baseline: Option<PathStyle>) -> S {
    let mut last_gid = None;
    let mut offset = Vector::default();
    let glyphs: Vec<_> = text.chars()
        .map(|c| font.gid_for_unicode_codepoint(c as u32).unwrap_or(font.get_notdef_gid()))
        .filter_map(|gid| font.glyph(gid).map(|glyph| (gid, glyph)))
        .map(|(gid, glyph)| {
            if let Some(left) = last_gid.replace(gid) {
                offset = offset + Vector::new(dbg!(font.kerning(left, gid)), 0.0);
            }
            let p = offset - glyph.metrics.lsb;
            offset = offset + glyph.metrics.advance;
            (glyph, p)
        })
        .collect();
    
    let bbox = font.bbox().expect("no bbox");
    let origin = Vector::new(0., -bbox.origin().y());
    let width = (offset.x()) * font.font_matrix().m11();
    let height = bbox.size().y() * font.font_matrix().m22();
    let mut surface = S::new(Vector::new(width * font_size, font_size * height));
    
    let tr = Transform::from_scale(Vector::splat(font_size))
            * Transform::from_translation(Vector::new(0., height))
            * Transform::from_scale(Vector::new(1.0, -1.0))
            * font.font_matrix();
    
    if let Some(style) = baseline {
        let style = surface.build_style(style);
        let mut p = PathBuilder::new();
        p.move_to(origin);
        p.line_to(origin + offset);
        let o: S::Outline = p.into_outline();
        surface.draw_path(o.transform(tr), &style);
    }
    let style = surface.build_style(style);
    for (glyph, p) in glyphs {
        let transform = tr * Transform::from_translation(p + origin);
        surface.draw_path(glyph.path.transform(transform), &style);
    }
    
    surface
}

mod truetype;
mod cff;
mod type1;
mod type2;
mod postscript;
mod opentype;
mod parsers;
mod eexec;
mod woff;

pub use truetype::TrueTypeFont;
pub use cff::CffFont;
pub use type1::Type1Font;
pub use opentype::parse_opentype;
use woff::{woff, woff2};

pub type R<'a, T> = IResult<&'a [u8], T, VerboseError<&'a [u8]>>;

#[derive(Copy, Clone)]
pub enum Value {
    Int(i32),
    Float(f32)
}
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => i.fmt(f),
            Value::Float(x) => x.fmt(f)
        }
    }
}

impl Into<f32> for Value {
    fn into(self) -> f32 {
        self.to_float()
    }
}
impl From<i16> for Value {
    fn from(v: i16) -> Value {
        Value::Int(v as i32)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::Int(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::Float(v)
    }
}
impl Value {
    fn to_int(self) -> i32 {
        match self {
            Value::Int(i) => i,
            Value::Float(_) => panic!("tried to cast a float to int")
        }
    }
    fn to_uint(self) -> u32 {
        match self {
            Value::Int(i) if i >= 0 => i as u32,
            Value::Int(_) => panic!("expected a unsigned int"),
            Value::Float(_) => panic!("tried to cast a float to int")
        }
    }
    fn to_float(self) -> f32 {
        match self {
            Value::Int(i) => i as f32,
            Value::Float(f) => f
        }
    }
}

fn v(x: impl Into<f32>, y: impl Into<f32>) -> Vector {
    Vector::new(x.into(), y.into())
}

pub trait TryIndex {
    fn try_index(&self, idx: usize) -> Option<&[u8]>;
}
impl TryIndex for () {
    fn try_index(&self, idx: usize) -> Option<&[u8]> {
        None
    }
}
impl TryIndex for Vec<Vec<u8>> {
    fn try_index(&self, idx: usize) -> Option<&[u8]> {
        self.get(idx).map(|v| &**v)
    }
}
impl<'a> TryIndex for Vec<&'a [u8]> {
    fn try_index(&self, idx: usize) -> Option<&[u8]> {
        self.get(idx).map(|v| *v)
    }
}
impl<'a> TryIndex for &'a [&'a [u8]] {
    fn try_index(&self, idx: usize) -> Option<&[u8]> {
        self.get(idx).map(|v| *v)
    }
}
    

pub struct Context<T=(), U=()> {
    pub subr_bias: i32,
    pub subrs: T,
    pub global_subrs: U,
    pub global_subr_bias: i32,
}

impl<T, U> Context<T, U> where T: TryIndex, U: TryIndex {
    pub fn subr(&self, idx: i32) -> &[u8] {
        self.subrs.try_index((idx + self.subr_bias) as usize).expect("requested subroutine not found")
    }
    pub fn global_subr(&self, idx: i32) -> &[u8] {
        self.global_subrs.try_index((idx + self.global_subr_bias) as usize).expect("requested global subroutine not found")
    }
}

pub struct State<O: Outline> {
    pub stack: Vec<Value>,
    pub path: PathBuilder<O>,
    pub current: Vector,
    pub lsb: Option<Vector>,
    pub char_width: Option<f32>,
    pub done: bool,
    pub stem_hints: u32,
    pub delta_width: Option<f32>,
    pub first_stack_clearing_operator: bool
}

impl<O: Outline> State<O> {
    pub fn new() -> State<O> {
        State {
            stack: Vec::new(),
            path: PathBuilder::new(),
            current: Vector::default(),
            lsb: None,
            char_width: None,
            done: false,
            stem_hints: 0,
            delta_width: None,
            first_stack_clearing_operator: true
        }
    }
    pub fn into_path(self) -> O {
        self.path.into_outline()
    }
    pub fn push(&mut self, v: impl Into<Value>) {
        self.stack.push(v.into());
    }
    pub fn pop(&mut self) -> Value {
        self.stack.pop().expect("no value on the stack")
    }
    /// get stack[0 .. T::N] as a tuple
    /// does not modify the stack
    pub fn args<T>(&mut self) -> T where
        T: TupleElements<Element=Value>
    {
        debug!("get {} args from {:?}", T::N, self.stack);
        T::from_iter(self.stack.iter().cloned()).unwrap()
    }
}

pub trait IResultExt {
    type Item;
    fn get(self) -> Self::Item;
}
impl<T> IResultExt for IResult<&[u8], T, VerboseError<&[u8]>> {
    type Item = T;
    fn get(self) -> T {
        match self {
            Ok((_, t)) => t,
            Err(Incomplete(_)) => panic!("need more data"),
            Err(Error(v)) | Err(Failure(v)) => {
                for (i, e) in v.errors {
                    println!("{:?} {:?}", &i[.. i.len().min(20)], e);
                    println!("{:?}", String::from_utf8_lossy(&i[.. i.len().min(20)]));
                }
                panic!()
            }
        }
    }
}

pub fn parse<O: Outline + 'static>(data: &[u8]) -> Box<dyn Font<O>> {
    let magic: &[u8; 4] = data[0 .. 4].try_into().unwrap();
    info!("font magic: {:?}", magic);
    match magic {
        &[0x80, 1, _, _] => Box::new(Type1Font::parse_pfb(data)) as _,
        b"OTTO" | [0,1,0,0] => parse_opentype(data, 0),
        b"ttcf" | b"typ1" => unimplemented!(), // Box::new(TrueTypeFont::parse(data, 0)) as _,
        b"true" => Box::new(TrueTypeFont::parse(data)) as _,
        b"%!PS" => Box::new(Type1Font::parse_postscript(data)) as _,
        b"wOFF" => woff(data),
        b"wOF2" => woff2(data),
        &[1, _, _, _] => Box::new(CffFont::parse(data, 0)) as _,
        magic => panic!("unknown magic {:?}", magic)
    }
}
