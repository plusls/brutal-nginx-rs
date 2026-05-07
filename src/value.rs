use std::{
    ptr::NonNull,
    str::{FromStr, ParseBoolError},
};

use nginx_sys::{
    NGX_OK, ngx_conf_t, ngx_connection_t, ngx_http_compile_complex_value,
    ngx_http_compile_complex_value_t, ngx_http_complex_value_t, ngx_int_t, ngx_log_t, ngx_str_t,
    ngx_stream_compile_complex_value, ngx_stream_compile_complex_value_t,
    ngx_stream_complex_value_t,
};
use ngx::{core::NgxStr, http::Request, log::DebugMask};

use crate::stream::Session;

#[derive(Debug, Clone)]
pub enum NginxValue<T, CVT> {
    Complex(NginxComplexValue<CVT>),
    Static(T),
}

#[derive(Debug, Clone)]
pub struct NginxComplexValue<CVT> {
    pub cv: NonNull<CVT>,
    pub cmd_name: String,
    pub file_name: String,
    pub line: u32,
}

pub trait ComplexValueTrait {
    type NginxCompileComplexValueType;
    fn init_compile_complex_value(
        compile_complex_value: &mut Self::NginxCompileComplexValueType,
        cf: *mut ngx_conf_t,
        value: *mut ngx_str_t,
        complex_value: *mut Self,
    );
    fn compile(
        compile_complex_value: &mut Self::NginxCompileComplexValueType,
    ) -> Result<(), String>;
    fn static_value<T: FromStr>(&self) -> Result<Option<T>, String>;
}
impl ComplexValueTrait for ngx_stream_complex_value_t {
    type NginxCompileComplexValueType = ngx_stream_compile_complex_value_t;
    fn init_compile_complex_value(
        compile_complex_value: &mut Self::NginxCompileComplexValueType,
        cf: *mut ngx_conf_t,
        value: *mut ngx_str_t,
        complex_value: *mut Self,
    ) {
        compile_complex_value.cf = cf;
        compile_complex_value.value = value;
        compile_complex_value.complex_value = complex_value;
    }

    fn compile(
        compile_complex_value: &mut Self::NginxCompileComplexValueType,
    ) -> Result<(), String> {
        if unsafe { ngx_stream_compile_complex_value(compile_complex_value) } != NGX_OK as ngx_int_t
        {
            return Err("ngx_stream_compile_complex_value failed.".to_string());
        }
        Ok(())
    }

    fn static_value<T: FromStr>(&self) -> Result<Option<T>, String> {
        if self.lengths.is_null() {
            Ok(Some(parse_nginx_str(unsafe {
                NgxStr::from_ngx_str(self.value)
            })?))
        } else {
            Ok(None)
        }
    }
}

impl ComplexValueTrait for ngx_http_complex_value_t {
    type NginxCompileComplexValueType = ngx_http_compile_complex_value_t;
    fn init_compile_complex_value(
        compile_complex_value: &mut Self::NginxCompileComplexValueType,
        cf: *mut ngx_conf_t,
        value: *mut ngx_str_t,
        complex_value: *mut Self,
    ) {
        compile_complex_value.cf = cf;
        compile_complex_value.value = value;
        compile_complex_value.complex_value = complex_value;
    }
    fn compile(
        compile_complex_value: &mut Self::NginxCompileComplexValueType,
    ) -> Result<(), String> {
        if unsafe { ngx_http_compile_complex_value(compile_complex_value) } != NGX_OK as ngx_int_t {
            return Err("ngx_http_compile_complex_value failed.".to_string());
        }
        Ok(())
    }

    fn static_value<T: FromStr>(&self) -> Result<Option<T>, String> {
        if self.lengths.is_null() {
            Ok(Some(parse_nginx_str(unsafe {
                NgxStr::from_ngx_str(self.value)
            })?))
        } else {
            Ok(None)
        }
    }
}

fn parse_nginx_str<T: FromStr>(s: &NgxStr) -> Result<T, String> {
    let s = s
        .to_str()
        .map_err(|_| "not utf-8 encoded nginx str".to_string())?;
    s.parse::<T>().map_err(|_| format!("cannot parse `{s}`"))
}

pub trait NginxHandlerCtxTrait {
    type NginxComplexValueType;
    const DEBUG_MASK: DebugMask;
    fn get_complex_value<T: FromStr>(
        &mut self,
        cv: &mut NginxComplexValue<Self::NginxComplexValueType>,
    ) -> Result<T, String> {
        if let Some(vs) = self.get_nginx_complex_value(unsafe { cv.cv.as_mut() }) {
            parse_nginx_str(vs).map_err(|err| {
                format!(
                    "Cannot parse `{}` at file: {}, line: {}, err: {err}",
                    cv.cmd_name, cv.file_name, cv.line
                )
            })
        } else {
            Err(format!(
                "Cannot get_complex_value for `{}` at file: {}, line: {}",
                cv.cmd_name, cv.file_name, cv.line,
            ))
        }
    }
    fn get_nginx_complex_value(&mut self, cv: &mut Self::NginxComplexValueType) -> Option<&NgxStr>;
    fn log(&self) -> *mut ngx_log_t;
    fn connection(&self) -> &ngx_connection_t;
}

impl NginxHandlerCtxTrait for Session {
    type NginxComplexValueType = ngx_stream_complex_value_t;
    const DEBUG_MASK: DebugMask = DebugMask::Stream;

    fn get_nginx_complex_value(&mut self, cv: &mut Self::NginxComplexValueType) -> Option<&NgxStr> {
        self.get_complex_value(cv)
    }
    fn log(&self) -> *mut ngx_log_t {
        self.log()
    }
    fn connection(&self) -> &ngx_connection_t {
        let conn = self.connection();
        unsafe { conn.as_mut() }.expect("connection always not null")
    }
}

impl NginxHandlerCtxTrait for Request {
    type NginxComplexValueType = ngx_http_complex_value_t;
    const DEBUG_MASK: DebugMask = DebugMask::Http;

    fn get_nginx_complex_value(&mut self, cv: &mut Self::NginxComplexValueType) -> Option<&NgxStr> {
        Request::get_complex_value(self, cv)
    }
    fn log(&self) -> *mut ngx_log_t {
        self.log()
    }
    fn connection(&self) -> &ngx_connection_t {
        let conn = self.connection();
        unsafe { conn.as_mut() }.expect("connection always not null")
    }
}

impl<CVT> NginxComplexValue<CVT> {
    pub fn extract<T: Clone + FromStr, CTX: NginxHandlerCtxTrait<NginxComplexValueType = CVT>>(
        &mut self,
        ctx: Option<&mut CTX>,
    ) -> Result<T, String> {
        if let Some(ctx) = ctx {
            ctx.get_complex_value::<T>(self)
        } else {
            Err(format!(
                "Cannot parse `{}` at file: {}, line: {} with None ctx",
                self.cmd_name, self.file_name, self.line
            ))
        }
    }
}

impl<T: Clone + FromStr, CVT> NginxValue<T, CVT> {
    pub fn extract<CTX: NginxHandlerCtxTrait<NginxComplexValueType = CVT>>(
        &mut self,
        ctx: Option<&mut CTX>,
    ) -> Result<T, String> {
        match self {
            NginxValue::Complex(value, ..) => value.extract(ctx),
            NginxValue::Static(v) => Ok(v.clone()),
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
