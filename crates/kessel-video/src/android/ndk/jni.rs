//! Just enough JNI to read the CAMERA permission:
//! `context.checkSelfPermission("android.permission.CAMERA")`. The JNI
//! function tables are read by their fixed slots in the JNI specification,
//! so no binding crate is needed.

use std::ffi::{c_char, c_void, CStr};

type JavaVm = *mut *const c_void;
type JniEnv = *mut *const c_void;
type JObject = *mut c_void;

const JNI_OK: i32 = 0;
const JNI_EDETACHED: i32 = -2;
const JNI_VERSION_1_6: i32 = 0x0001_0006;
const PERMISSION_GRANTED: i32 = 0;

// Slots in `JNIInvokeInterface` (the JavaVM table).
const VM_ATTACH_CURRENT_THREAD: usize = 4;
const VM_DETACH_CURRENT_THREAD: usize = 5;
const VM_GET_ENV: usize = 6;
// Slots in `JNINativeInterface` (the JNIEnv table).
const ENV_EXCEPTION_CLEAR: usize = 17;
const ENV_DELETE_LOCAL_REF: usize = 23;
const ENV_GET_OBJECT_CLASS: usize = 31;
const ENV_GET_METHOD_ID: usize = 33;
const ENV_CALL_INT_METHOD_A: usize = 51;
const ENV_NEW_STRING_UTF: usize = 167;
const ENV_EXCEPTION_CHECK: usize = 228;

/// A JNI `jvalue` (the union of every Java type), for the `...A` calls.
#[repr(C)]
#[derive(Clone, Copy)]
union JValue {
    l: JObject,
    /// Never read: it sizes the union to `jvalue`'s 8 bytes on 32-bit
    /// targets too.
    #[allow(dead_code)]
    j: i64,
}

/// Function `index` of the table at `*table_holder`, as `F`.
///
/// # Safety
///
/// `table_holder` must be a live `JavaVM*` or `JNIEnv*`, `index` a slot of
/// its table, and `F` the function pointer type of that slot.
unsafe fn func<F: Copy>(table_holder: *mut *const c_void, index: usize) -> F {
    assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*const c_void>(),
        "a JNI slot is one function pointer"
    );
    // SAFETY: the caller's promise: a JavaVM or JNIEnv points at its
    // function table, whose slots are pointer-sized and fixed by the spec,
    // and `F` is the slot's function type.
    unsafe {
        let p = *(*table_holder as *const *const c_void).add(index);
        std::mem::transmute_copy::<*const c_void, F>(&p)
    }
}

/// An env for this thread, attaching it if needed (detached again on
/// drop).
struct Env {
    vm: JavaVm,
    env: JniEnv,
    attached: bool,
}

impl Env {
    /// # Safety
    ///
    /// `vm` must be the process's live `JavaVM*`.
    unsafe fn get(vm: JavaVm) -> Option<Env> {
        type GetEnv = unsafe extern "system" fn(JavaVm, *mut JniEnv, i32) -> i32;
        type Attach = unsafe extern "system" fn(JavaVm, *mut JniEnv, *mut c_void) -> i32;
        let mut env: JniEnv = std::ptr::null_mut();
        // SAFETY: `vm` is live; slots 6 and 4 are GetEnv and
        // AttachCurrentThread, with these signatures.
        unsafe {
            let get_env: GetEnv = func(vm, VM_GET_ENV);
            match get_env(vm, &mut env, JNI_VERSION_1_6) {
                JNI_OK if !env.is_null() => Some(Env {
                    vm,
                    env,
                    attached: false,
                }),
                JNI_EDETACHED => {
                    let attach: Attach = func(vm, VM_ATTACH_CURRENT_THREAD);
                    (attach(vm, &mut env, std::ptr::null_mut()) == JNI_OK && !env.is_null())
                        .then_some(Env {
                            vm,
                            env,
                            attached: true,
                        })
                }
                _ => None,
            }
        }
    }

    /// Clear a pending Java exception; true if there was one.
    fn clear_exception(&self) -> bool {
        type Check = unsafe extern "system" fn(JniEnv) -> u8;
        type Clear = unsafe extern "system" fn(JniEnv);
        // SAFETY: `env` is this thread's live env; slots 228 and 17 are
        // ExceptionCheck and ExceptionClear.
        unsafe {
            let check: Check = func(self.env, ENV_EXCEPTION_CHECK);
            if check(self.env) == 0 {
                return false;
            }
            let clear: Clear = func(self.env, ENV_EXCEPTION_CLEAR);
            clear(self.env);
        }
        true
    }

    fn delete_local(&self, obj: JObject) {
        type Delete = unsafe extern "system" fn(JniEnv, JObject);
        if obj.is_null() {
            return;
        }
        // SAFETY: slot 23 is DeleteLocalRef; `obj` is a local ref of this
        // env.
        unsafe {
            let delete: Delete = func(self.env, ENV_DELETE_LOCAL_REF);
            delete(self.env, obj);
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if self.attached {
            type Detach = unsafe extern "system" fn(JavaVm) -> i32;
            // SAFETY: slot 5 is DetachCurrentThread; this thread was
            // attached by `get`.
            unsafe {
                let detach: Detach = func(self.vm, VM_DETACH_CURRENT_THREAD);
                detach(self.vm);
            }
        }
    }
}

/// `context.checkSelfPermission("android.permission.CAMERA") ==
/// PERMISSION_GRANTED`, or `None` when it can't be asked.
pub(crate) fn camera_granted(vm: usize, context: usize) -> Option<bool> {
    type GetObjectClass = unsafe extern "system" fn(JniEnv, JObject) -> JObject;
    type GetMethodId =
        unsafe extern "system" fn(JniEnv, JObject, *const c_char, *const c_char) -> *mut c_void;
    type NewStringUtf = unsafe extern "system" fn(JniEnv, *const c_char) -> JObject;
    type CallIntMethodA =
        unsafe extern "system" fn(JniEnv, JObject, *mut c_void, *const JValue) -> i32;
    let context = context as JObject;
    // SAFETY: the host gave a live JavaVM* (AndroidContext::new's contract).
    let env = unsafe { Env::get(vm as JavaVm)? };
    let e = env.env;
    let name: &CStr = c"checkSelfPermission";
    let sig: &CStr = c"(Ljava/lang/String;)I";
    let perm: &CStr = c"android.permission.CAMERA";
    // SAFETY: `e` is this thread's env; each slot holds the function its
    // type names (JNI spec, table order); `context` is a global ref to a
    // Context (AndroidContext::new's contract); local refs are deleted
    // below and exceptions cleared after each call that can throw.
    unsafe {
        let get_class: GetObjectClass = func(e, ENV_GET_OBJECT_CLASS);
        let class = get_class(e, context);
        if class.is_null() || env.clear_exception() {
            return None;
        }
        let get_method: GetMethodId = func(e, ENV_GET_METHOD_ID);
        let method = get_method(e, class, name.as_ptr(), sig.as_ptr());
        env.delete_local(class);
        if method.is_null() || env.clear_exception() {
            return None;
        }
        let new_string: NewStringUtf = func(e, ENV_NEW_STRING_UTF);
        let jperm = new_string(e, perm.as_ptr());
        if jperm.is_null() || env.clear_exception() {
            return None;
        }
        let call: CallIntMethodA = func(e, ENV_CALL_INT_METHOD_A);
        let args = [JValue { l: jperm }];
        let result = call(e, context, method, args.as_ptr());
        let threw = env.clear_exception();
        env.delete_local(jperm);
        (!threw).then_some(result == PERMISSION_GRANTED)
    }
}
