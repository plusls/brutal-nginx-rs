use std::{alloc::Layout, ptr::NonNull, str::FromStr};

use nginx_sys::{
    __socket_type_SOCK_STREAM, AF_UNIX, IPPROTO_TCP, NGX_LOG_ERR, TCP_CONGESTION, setsockopt,
};
use ngx::{
    allocator::Allocator,
    core::{Pool, Status},
    ffi::{NGX_LOG_EMERG, ngx_command_t, ngx_conf_t, ngx_str_t},
    http::{Merge, MergeConfigError},
    ngx_conf_log_error, ngx_log_debug, ngx_log_error,
};

use crate::value::{
    ComplexValueTrait, NginxBool, NginxComplexValue, NginxHandlerCtxTrait, NginxValue,
};

pub mod value;

#[cfg(ngx_feature = "stream")]
pub mod stream;

pub struct Module;

#[derive(Debug)]
pub struct ModuleConfig<CVT> {
    pub enable: Option<NginxValue<NginxBool, CVT>>,
    pub rate: Option<NginxValue<u64, CVT>>,
    pub cwnd_gain: Option<NginxValue<u32, CVT>>,
}

impl<CVT> Default for ModuleConfig<CVT> {
    fn default() -> Self {
        Self {
            enable: Default::default(),
            rate: Default::default(),
            cwnd_gain: Default::default(),
        }
    }
}

impl<CVT> ModuleConfig<CVT> {
    pub fn get_brutal_param<CTX: NginxHandlerCtxTrait<NginxComplexValueType = CVT>>(
        &mut self,
        ctx: &mut CTX,
    ) -> Option<BrutalParams> {
        if let Some(enable) = &mut self.enable {
            match enable.extract(Some(ctx)) {
                Ok(enable) => {
                    if !enable.as_bool() {
                        return None;
                    }
                }
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, ctx.log(), "{err}",);
                }
            }
        } else {
            return None;
        }

        if let Some(rate) = &mut self.rate {
            let mut cwnd_gain_default = NginxValue::Static(15);
            let cwnd_gain = self.cwnd_gain.as_mut().unwrap_or(&mut cwnd_gain_default);

            let rate = match rate.extract(Some(ctx)) {
                Ok(rate) => rate,
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, ctx.log(), "{err}",);
                    return None;
                }
            };

            let cwnd_gain = match cwnd_gain.extract(Some(ctx)) {
                Ok(cwnd_gain) => cwnd_gain,
                Err(err) => {
                    ngx_log_error!(NGX_LOG_EMERG, ctx.log(), "{err}",);
                    return None;
                }
            };

            Some(BrutalParams { rate, cwnd_gain })
        } else {
            None
        }
    }
}

#[repr(C, packed)]
// TCP_BRUTAL_PARAMS expects the packed 12-byte kernel ABI: u64 rate + u32 cwnd_gain.
pub struct BrutalParams {
    rate: u64,
    cwnd_gain: u32,
}

impl<CVT: Clone> Merge for ModuleConfig<CVT> {
    fn merge(&mut self, prev: &ModuleConfig<CVT>) -> Result<(), MergeConfigError> {
        if self.enable.is_none() {
            self.enable = prev.enable.clone();
        }
        if self.rate.is_none() {
            self.rate = prev.rate.clone();
        };
        if self.cwnd_gain.is_none() {
            self.cwnd_gain = prev.cwnd_gain.clone();
        };
        Ok(())
    }
}

pub fn brutal_handler<T: NginxHandlerCtxTrait>(
    ctx: &mut T,
    brutal_param: Option<BrutalParams>,
) -> Status {
    let enable = brutal_param.is_some();
    let connection = ctx.connection();
    let should_apply_brutal = connection.type_ as u32 == __socket_type_SOCK_STREAM
        && unsafe {
            connection
                .listening
                .as_ref()
                .and_then(|listening| listening.sockaddr.as_ref())
                .is_some_and(|sockaddr| sockaddr.sa_family as u32 != AF_UNIX)
        };

    if let Some(brutal_param) = brutal_param
        && should_apply_brutal
    {
        let fd = connection.fd;

        let log = ctx.log();
        ngx_log_debug!(mask: <T as NginxHandlerCtxTrait>::DEBUG_MASK, log, "brutal module enabled: {enable}");

        let algo = b"brutal\0";

        let set_sock_opt_ret = unsafe {
            setsockopt(
                fd,
                IPPROTO_TCP as i32,
                TCP_CONGESTION as i32,
                algo.as_ptr().cast(),
                algo.len() as u32,
            )
        };
        if set_sock_opt_ret != 0 {
            ngx_log_error!(
                NGX_LOG_ERR,
                log,
                "tcp brutal TCP_CONGESTION 1 error: {set_sock_opt_ret}"
            );
            return Status::NGX_DECLINED;
        }

        const TCP_BRUTAL_PARAMS: u32 = 23301;
        let set_sock_opt_ret = unsafe {
            setsockopt(
                fd,
                IPPROTO_TCP as i32,
                TCP_BRUTAL_PARAMS as i32,
                (&brutal_param as *const BrutalParams).cast(),
                std::mem::size_of::<BrutalParams>() as u32,
            )
        };
        if set_sock_opt_ret != 0 {
            ngx_log_error!(
                NGX_LOG_ERR,
                log,
                "tcp brutal TCP_CONGESTION 2 error: {set_sock_opt_ret}"
            );
            return Status::NGX_DECLINED;
        }
        Status::NGX_OK
    } else {
        Status::NGX_DECLINED
    }
}

pub fn extract_nginx_value<T: FromStr, CVT: ComplexValueTrait>(
    mut cf: NonNull<ngx_conf_t>,
    cmd: NonNull<ngx_command_t>,
) -> Option<NginxValue<T, CVT>> {
    let args: &mut [ngx_str_t] = unsafe { (*cf.as_ref().args).as_slice_mut() };
    let cf = unsafe { cf.as_mut() };
    let cmd_name = unsafe { cmd.as_ref() }.name;
    let cmd_name = if let Ok(cmd_name) = cmd_name.to_str() {
        cmd_name
    } else {
        ngx_conf_log_error!(NGX_LOG_EMERG, cf, "cmd name {} not utf-8 encoded", cmd_name);
        return None;
    };

    let pool = unsafe { Pool::from_ngx_pool(cf.pool) };
    let layout = Layout::new::<CVT>();
    let cv = pool.allocate(unsafe { std::mem::zeroed::<CVT>() });
    let mut cv = match NonNull::new(cv) {
        Some(cv) => cv,
        None => {
            ngx_conf_log_error!(
                NGX_LOG_EMERG,
                cf,
                "failed to allocate ngx complex_value_t for `{cmd_name}`"
            );
            return None;
        }
    };

    let mut ccv: CVT::NginxCompileComplexValueType = unsafe { std::mem::zeroed() };
    CVT::init_compile_complex_value(&mut ccv, cf, &mut args[1] as _, unsafe { cv.as_mut() }
        as *mut _);

    if let Err(err) = CVT::compile(&mut ccv) {
        ngx_conf_log_error!(
            NGX_LOG_EMERG,
            cf,
            "`{cmd_name}` argument compile failed. err: {err}"
        );
        return None;
    }

    match CVT::static_value::<T>(unsafe { cv.as_ref() }) {
        Ok(Some(static_value)) => {
            // 静态值
            unsafe {
                pool.deallocate(cv.cast::<u8>(), layout);
            }
            Some(NginxValue::Static(static_value))
        }
        Err(err) => {
            // 静态值解析失败
            unsafe {
                pool.deallocate(cv.cast::<u8>(), layout);
            }
            ngx_conf_log_error!(NGX_LOG_EMERG, cf, "invalid {cmd_name}, err: {err}",);
            None
        }
        Ok(None) => {
            let conf_file = cf.conf_file;
            let (file_name, line) = if let Some(conf_file) = NonNull::new(conf_file) {
                let conf_file = unsafe { conf_file.as_ref() };
                let file_name = if let Ok(file_name) = conf_file.file.name.to_str() {
                    file_name.to_string()
                } else {
                    ngx_conf_log_error!(
                        NGX_LOG_EMERG,
                        cf,
                        "invalid config file name {}",
                        conf_file.file.name
                    );
                    return None;
                };
                (file_name, conf_file.line as u32)
            } else {
                ngx_conf_log_error!(NGX_LOG_EMERG, cf, "conf_file name is null",);
                return None;
            };
            // complex value，里面有变量
            let val = NginxValue::Complex(NginxComplexValue {
                cv,
                cmd_name: cmd_name.to_string(),
                file_name,
                line,
            });
            Some(val)
        }
    }
}
