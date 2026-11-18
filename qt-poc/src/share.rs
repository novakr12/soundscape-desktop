#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("share.h");
        #[cxx_name = "shareGlContexts"]
        fn share_gl_contexts();
    }
}
