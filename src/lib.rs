#![cfg_attr(feature="clippy", feature(plugin))]
#![cfg_attr(feature="clippy", plugin(clippy))]
#![cfg_attr(feature="clippy", warn(clippy))]

#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate lazy_static;

extern crate libc;

use std::mem;
use std::marker::{PhantomData};
use std::path::{Path};
use std::sync::atomic::{AtomicBool, Ordering};

use libc::{c_int, c_uint, c_ulong};

pub mod ffi;

//================================================
// Macros
//================================================

// options! ______________________________________

macro_rules! options {
    ($(#[$attribute:meta])* options $name:ident: $underlying:ident {
        $($(#[$fattribute:meta])* pub $option:ident: $flag:ident), +,
    }) => (
        $(#[$attribute])*
        #[derive(Default)]
        pub struct $name {
            $($(#[$fattribute])* pub $option: bool), +,
        }

        impl Into<ffi::$underlying> for $name {
            fn into(self) -> ffi::$underlying {
                let mut flags = ffi::$underlying::empty();
                $(if self.$option { flags.insert(ffi::$flag); })+
                flags
            }
        }
    );
}

//================================================
// Enums
//================================================

// SaveError _____________________________________

/// Indicates the type of error that prevented the saving of a translation unit to an AST file.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SaveError {
    /// Errors in the translation unit prevented saving.
    Errors,
    /// An unknown error occurred.
    Unknown,
}

// SourceError ___________________________________

/// Indicates the type of error that prevented the loading of a translation unit from a source file.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceError {
    /// An error occurred while deserializing an AST file.
    AstDeserialization,
    /// `libclang` crashed.
    Crash,
    /// An unknown error occurred.
    Unknown,
}

//================================================
// Structs
//================================================

// Clang _________________________________________

lazy_static! { static ref AVAILABLE: AtomicBool = AtomicBool::new(true); }

/// An empty type which prevents the use of this library from multiple threads.
pub struct Clang;

impl Clang {
    //- Constructors -----------------------------

    /// Constructs a new `Clang`.
    ///
    /// Only one instance of `Clang` is allowed at a time.
    ///
    /// # Failures
    ///
    /// * an instance of `Clang` already exists
    pub fn new() -> Result<Clang, ()> {
        if AVAILABLE.swap(false, Ordering::Relaxed) {
            Ok(Clang)
        } else {
            Err(())
        }
    }
}

impl Drop for Clang {
    fn drop(&mut self) {
        AVAILABLE.store(true, Ordering::Relaxed);
    }
}

// Index _________________________________________

/// Indicates which types of threads have background priority.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BackgroundPriority {
    pub editing: bool,
    pub indexing: bool,
}

/// A collection of translation units.
pub struct Index<'c> {
    handle: ffi::CXIndex,
    _marker: PhantomData<&'c Clang>,
}

impl<'c> Index<'c> {
    //- Constructors -----------------------------

    /// Constructs a new `Index`.
    ///
    /// `exclude` determines whether declarations from precompiled headers are excluded and
    /// `diagnostics` determines whether diagnostics are printed while parsing source files.
    pub fn new(_: &'c Clang, exclude: bool, diagnostics: bool) -> Index<'c> {
        let handle = unsafe { ffi::clang_createIndex(exclude as c_int, diagnostics as c_int) };
        Index { handle: handle, _marker: PhantomData }
    }

    //- Accessors --------------------------------

    /// Returns which types of threads have background priority.
    pub fn get_background_priority(&self) -> BackgroundPriority {
        let flags = unsafe { ffi::clang_CXIndex_getGlobalOptions(self.handle) };
        let editing = flags.contains(ffi::CXGlobalOpt_ThreadBackgroundPriorityForEditing);
        let indexing = flags.contains(ffi::CXGlobalOpt_ThreadBackgroundPriorityForIndexing);
        BackgroundPriority { editing: editing, indexing: indexing }
    }

    //- Mutators ---------------------------------

    /// Sets which types of threads have background priority.
    pub fn set_background_priority(&mut self, priority: BackgroundPriority) {
        let mut flags = ffi::CXGlobalOptFlags::empty();

        if priority.editing {
            flags.insert(ffi::CXGlobalOpt_ThreadBackgroundPriorityForEditing);
        }

        if priority.indexing {
            flags.insert(ffi::CXGlobalOpt_ThreadBackgroundPriorityForIndexing);
        }

        unsafe { ffi::clang_CXIndex_setGlobalOptions(self.handle, flags); }
    }
}

impl<'c> Drop for Index<'c> {
    fn drop(&mut self) {
        unsafe { ffi::clang_disposeIndex(self.handle); }
    }
}

// ParseOptions __________________________________

options! {
    /// A set of options that determines how a source file is parsed into a translation unit.
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    options ParseOptions: CXTranslationUnit_Flags {
        /// Indicates whether certain code completion results will be cached when the translation
        /// unit is reparsed.
        ///
        /// This option increases the time it takes to reparse the translation unit but improves
        /// code completion performance.
        pub cache_completion_results: CXTranslationUnit_CacheCompletionResults,
        /// Indicates whether a detailed preprocessing record will be constructed which includes all
        /// macro definitions and instantiations.
        pub detailed_preprocessing_record: CXTranslationUnit_DetailedPreprocessingRecord,
        /// Indicates whether brief documentation comments will be included in code completion
        /// results.
        pub include_brief_comments_in_code_completion: CXTranslationUnit_IncludeBriefCommentsInCodeCompletion,
        /// Indicates whether the translation unit will be considered incomplete.
        ///
        /// This option suppresses certain semantic analyses and is typically used when parsing
        /// headers with the intent of creating a precompiled header.
        pub incomplete: CXTranslationUnit_Incomplete,
        /// Indicates whether function and method bodies will be skipped.
        pub skip_function_bodies: CXTranslationUnit_SkipFunctionBodies,
    }
}

// TranslationUnit _______________________________

/// A preprocessed and parsed source file.
pub struct TranslationUnit<'i> {
    handle: ffi::CXTranslationUnit,
    _marker: PhantomData<&'i Index<'i>>,
}

impl<'i> TranslationUnit<'i> {
    //- Constructors -----------------------------

    fn from_ptr(handle: ffi::CXTranslationUnit) -> TranslationUnit<'i> {
        TranslationUnit{ handle: handle, _marker: PhantomData }
    }

    /// Constructs a new `TranslationUnit` from an AST file.
    ///
    /// # Failures
    ///
    /// * an unknown error occurs
    pub fn from_ast<F: AsRef<Path>>(
        index: &'i mut Index, file: F
    ) -> Result<TranslationUnit<'i>, ()> {
        let handle = unsafe {
            ffi::clang_createTranslationUnit(index.handle, from_path(file).as_ptr())
        };

        if !handle.0.is_null() {
            Ok(TranslationUnit::from_ptr(handle))
        } else {
            Err(())
        }
    }

    /// Constructs a new `TranslationUnit` from a source file.
    ///
    /// Any compiler argument that may be supplied to `clang` may be supplied to this function.
    /// However, the following arguments are ignored:
    ///
    /// * `-c`
    /// * `-emit-ast`
    /// * `-fsyntax-only`
    /// * `-o` and the following `<output>`
    ///
    /// # Failures
    ///
    /// * an error occurs while deserializing an AST file
    /// * `libclang` crashes
    /// * an unknown error occurs
    pub fn from_source<F: AsRef<Path>>(
        index: &'i mut Index,
        file: F,
        arguments: &[&str],
        unsaved: &[Unsaved],
        options: ParseOptions,
    ) -> Result<TranslationUnit<'i>, SourceError> {
        let arguments = arguments.iter().map(|a| from_string(a)).collect::<Vec<_>>();
        let arguments = arguments.iter().map(|a| a.as_ptr()).collect::<Vec<_>>();
        let unsaved = unsaved.iter().map(|u| u.as_raw()).collect::<Vec<_>>();

        unsafe {
            let mut handle = mem::uninitialized();

            let code = ffi::clang_parseTranslationUnit2(
                index.handle,
                from_path(file).as_ptr(),
                arguments.as_ptr(),
                arguments.len() as c_int,
                mem::transmute(unsaved.as_ptr()),
                unsaved.len() as c_uint,
                options.into(),
                &mut handle,
            );

            match code {
                ffi::CXErrorCode::Success => Ok(TranslationUnit::from_ptr(handle)),
                ffi::CXErrorCode::ASTReadError => Err(SourceError::AstDeserialization),
                ffi::CXErrorCode::Crashed => Err(SourceError::Crash),
                ffi::CXErrorCode::Failure => Err(SourceError::Unknown),
                _ => unreachable!(),
            }
        }
    }

    //- Accessors --------------------------------

    /// Saves this translation unit to an AST file.
    ///
    /// # Failures
    ///
    /// * errors in the translation unit prevent saving
    /// * an unknown error occurs
    pub fn save<F: AsRef<Path>>(&self, file: F) -> Result<(), SaveError> {
        let code = unsafe {
            ffi::clang_saveTranslationUnit(
                self.handle, from_path(file).as_ptr(), ffi::CXSaveTranslationUnit_None
            )
        };

        match code {
            ffi::CXSaveError::None => Ok(()),
            ffi::CXSaveError::InvalidTU => Err(SaveError::Errors),
            ffi::CXSaveError::Unknown => Err(SaveError::Unknown),
            _ => unreachable!(),
        }
    }

    //- Consumers --------------------------------

    /// Consumes this translation unit and reparses the source file it was created from with the
    /// same compiler arguments that were used originally.
    ///
    /// # Failures
    ///
    /// * an error occurs while deserializing an AST file
    /// * `libclang` crashes
    /// * an unknown error occurs
    pub fn reparse(self, unsaved: &[Unsaved]) -> Result<TranslationUnit<'i>, SourceError> {
        let unsaved = unsaved.iter().map(|u| u.as_raw()).collect::<Vec<_>>();

        unsafe {
            let code = ffi::clang_reparseTranslationUnit(
                self.handle,
                unsaved.len() as c_uint,
                mem::transmute(unsaved.as_ptr()),
                ffi::CXReparse_None,
            );

            match code {
                ffi::CXErrorCode::Success => Ok(self),
                ffi::CXErrorCode::ASTReadError => Err(SourceError::AstDeserialization),
                ffi::CXErrorCode::Crashed => Err(SourceError::Crash),
                ffi::CXErrorCode::Failure => Err(SourceError::Unknown),
                _ => unreachable!(),
            }
        }
    }
}

impl<'i> Drop for TranslationUnit<'i> {
    fn drop(&mut self) {
        unsafe { ffi::clang_disposeTranslationUnit(self.handle); }
    }
}

// Unsaved _______________________________________

/// The path to and unsaved contents of a previously existing file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Unsaved {
    path: std::ffi::CString,
    contents: std::ffi::CString,
}

impl Unsaved {
    //- Constructors -----------------------------

    /// Constructs a new `Unsaved`.
    pub fn new<P: AsRef<Path>>(path: P, contents: &str) -> Unsaved {
        Unsaved { path: from_path(path), contents: from_string(contents) }
    }

    //- Accessors --------------------------------

    fn as_raw(&self) -> ffi::CXUnsavedFile {
        ffi::CXUnsavedFile {
            Filename: self.path.as_ptr(),
            Contents: self.contents.as_ptr(),
            Length: self.contents.as_bytes().len() as c_ulong,
        }
    }
}

//================================================
// Functions
//================================================

fn from_path<P>(path: P) -> std::ffi::CString where P: AsRef<Path> {
    from_string(path.as_ref().as_os_str().to_str().expect("invalid C string"))
}

fn from_string(string: &str) -> std::ffi::CString {
    std::ffi::CString::new(string).expect("invalid C string")
}
