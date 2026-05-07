use std::{
    ptr::NonNull,
    str::{FromStr, ParseBoolError},
};

use nginx_sys::ngx_stream_complex_value_t;

use crate::stream::Session;

#[derive(Debug, Clone)]
pub enum NginxStreamValue<T> {
    Complex(NginxStreamComplexValue),
    Static(T),
}

#[derive(Debug, Clone)]
pub struct NginxStreamComplexValue {
    pub cv: NonNull<ngx_stream_complex_value_t>,
    pub cmd_name: String,
    pub file_name: String,
    pub line: u32,
}

impl NginxStreamComplexValue {
    pub fn extract<T: Clone + FromStr>(
        &mut self,
        session: Option<&mut Session>,
    ) -> Result<T, String> {
        if let Some(session) = session {
            if let Some(vs) = session.get_complex_value(unsafe { self.cv.as_mut() }) {
                if let Ok(vs) = vs.to_str() {
                    vs.parse::<T>().map_err(|_| {
                        format!(
                            "Cannot parse `{}`: `{vs}` at file: {}, line: {}",
                            self.cmd_name, self.file_name, self.line
                        )
                    })
                } else {
                    Err(format!(
                        "Cannot parse `{}` at file: {}, line: {} with not utf-8 encoded nginx str {}",
                        self.cmd_name, self.file_name, self.line, vs
                    ))
                }
            } else {
                Err(format!(
                    "Cannot get_complex_value for `{}` at file: {}, line: {}",
                    self.cmd_name, self.file_name, self.line,
                ))
            }
        } else {
            Err(format!(
                "Cannot parse `{}` at file: {}, line: {} with None session",
                self.cmd_name, self.file_name, self.line
            ))
        }
    }
}

impl<T: Clone + FromStr> NginxStreamValue<T> {
    pub fn extract(&mut self, session: Option<&mut Session>) -> Result<T, String> {
        match self {
            NginxStreamValue::Complex(value, ..) => value.extract(session),
            NginxStreamValue::Static(v) => Ok(v.clone()),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NginxBool(bool);

impl FromStr for NginxBool {
    type Err = ParseBoolError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("on") {
            Ok(Self(true))
        } else if s.eq_ignore_ascii_case("off") {
            Ok(Self(false))
        } else {
            Ok(Self(s.parse::<bool>()?))
        }
    }
}

impl NginxBool {
    pub fn as_bool(self) -> bool {
        self.0
    }
}
