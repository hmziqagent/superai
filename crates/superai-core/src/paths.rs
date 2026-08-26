//! Path and executable reference types.
//!
//! All stored paths are normalized absolute forms without following symlinks.
//! Home or platform variable expansion is explicit at the adapter boundary
//! via [`AbsolutePath::expand_home`] and sibling helpers. Relative paths
//! containing `..` are always rejected. Symlink policy is handled by the
//! mutation layer, not by these types.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::CoreError;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn invalid_path(kind: &str, value: &str, reason: &str) -> CoreError {
    CoreError::InvalidPath {
        kind: kind.to_owned(),
        value: value.to_owned(),
        reason: reason.to_owned(),
    }
}

fn validate_not_empty(kind: &str, value: &str) -> Result<(), CoreError> {
    if value.is_empty() {
        return Err(invalid_path(kind, value, "must not be empty"));
    }
    Ok(())
}

fn validate_no_nul(kind: &str, value: &str) -> Result<(), CoreError> {
    if value.contains('\0') {
        return Err(invalid_path(kind, value, "must not contain NUL"));
    }
    Ok(())
}

fn validate_no_traversal(kind: &str, path: &Path, display: &str) -> Result<(), CoreError> {
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(invalid_path(kind, display, "must not contain '..'"));
        }
    }
    Ok(())
}

fn normalize_absolute(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir | Component::ParentDir => {}
            Component::Normal(segment) => out.push(segment),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Path::new("/"));
    }
    out
}

fn expand_tilde(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("~\\") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("${HOME}/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("%USERPROFILE%/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("%USERPROFILE%\\") {
        return home.join(rest);
    }
    PathBuf::from(value)
}

fn validate_absolute_path(kind: &str, path: &Path, display: &str) -> Result<PathBuf, CoreError> {
    validate_not_empty(kind, display)?;
    validate_no_nul(kind, display)?;
    if !path.is_absolute() {
        return Err(invalid_path(kind, display, "must be absolute"));
    }
    validate_no_traversal(kind, path, display)?;
    // Reject NUL in path's os string as well (handles non-UTF8)
    let lossy = path.to_string_lossy();
    if lossy.contains('\0') {
        return Err(invalid_path(kind, display, "must not contain NUL"));
    }
    Ok(normalize_absolute(path))
}

// ---------------------------------------------------------------------------
// AbsolutePath
// ---------------------------------------------------------------------------

/// Normalized absolute path without following symlinks.
///
/// Construction rejects empty, NUL, non-absolute, and any `..` component.
/// Lexical normalization removes `.` and duplicate separators but does not
/// canonicalize or follow links.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    /// Create from a string slice, validating and normalizing.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let path = Path::new(value);
        let normalized = validate_absolute_path("AbsolutePath", path, value)?;
        Ok(Self(normalized))
    }

    /// Create from a [`Path`] reference.
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let display = path.to_string_lossy();
        let display_str = display.as_ref();
        let normalized = validate_absolute_path("AbsolutePath", path, display_str)?;
        Ok(Self(normalized))
    }

    /// Expand a leading `~` or platform home variable using `home`, then
    /// validate and normalize.
    ///
    /// This is the adapter resolution boundary: raw strings containing
    /// `~`, `$HOME`, `${HOME}`, or `%USERPROFILE%` are expanded only here.
    /// The stored form is always absolute and normalized.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        validate_not_empty("AbsolutePath", value)?;
        if value.contains('\0') {
            return Err(invalid_path("AbsolutePath", value, "must not contain NUL"));
        }
        let expanded = expand_tilde(value, home);
        // Use original value for error display if expanded fails due to
        // traversal etc., but report the expanded display for absolute check.
        let display = expanded.to_string_lossy();
        let display_str = display.as_ref();
        // If expansion produced a path with `..`, from_path will reject.
        // Keep kind as AbsolutePath.
        let normalized = validate_absolute_path("AbsolutePath", &expanded, display_str)?;
        Ok(Self(normalized))
    }

    /// Instance variant of [`Self::expand_home`] for call sites that already
    /// hold an [`AbsolutePath`]. If `self` already is absolute, it is
    /// returned unchanged; if its string form starts with `~`, it is expanded.
    pub fn expand_home_ref(&self, home: &Path) -> Result<Self, CoreError> {
        let s = self.0.to_string_lossy();
        // Only expand if the stored form somehow contains a leading tilde
        // (defensive; normally AbsolutePath is already absolute).
        if s.starts_with('~')
            || s.starts_with("$HOME")
            || s.starts_with("${HOME}")
            || s.starts_with("%USERPROFILE%")
        {
            Self::expand_home(s.as_ref(), home)
        } else {
            Ok(self.clone())
        }
    }

    /// Borrow as [`Path`].
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Borrow as string lossy (for display).
    pub fn as_str_lossy(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    /// Consume into inner [`PathBuf`].
    pub fn into_inner(self) -> PathBuf {
        self.0
    }

    /// Join a relative segment, rejecting `..`, NUL, and absolute segments.
    pub fn join(&self, relative: &str) -> Result<Self, CoreError> {
        validate_not_empty("AbsolutePath", relative)?;
        validate_no_nul("AbsolutePath", relative)?;
        let rel_path = Path::new(relative);
        if rel_path.is_absolute() {
            return Err(invalid_path(
                "AbsolutePath",
                relative,
                "join segment must be relative",
            ));
        }
        validate_no_traversal("AbsolutePath", rel_path, relative)?;
        let joined = self.0.join(rel_path);
        // joined is absolute by construction, but re-validate traversal
        validate_no_traversal("AbsolutePath", &joined, relative)?;
        let normalized = normalize_absolute(&joined);
        Ok(Self(normalized))
    }
}

impl fmt::Display for AbsolutePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

impl AsRef<Path> for AbsolutePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<str> for AbsolutePath {
    fn as_ref(&self) -> &str {
        // We only construct from valid UTF-8 via `new(&str)`, but PathBuf
        // may contain non-UTF8 on some platforms. To avoid panic we return
        // empty string for non-UTF8. Callers needing the path should use
        // `as_path`.
        self.0.to_str().unwrap_or("")
    }
}

impl Deref for AbsolutePath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Borrow<Path> for AbsolutePath {
    fn borrow(&self) -> &Path {
        &self.0
    }
}

impl FromStr for AbsolutePath {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for AbsolutePath {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl TryFrom<&str> for AbsolutePath {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<PathBuf> for AbsolutePath {
    type Error = CoreError;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::from_path(&value)
    }
}

impl TryFrom<&Path> for AbsolutePath {
    type Error = CoreError;

    fn try_from(value: &Path) -> Result<Self, Self::Error> {
        Self::from_path(value)
    }
}

impl Serialize for AbsolutePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string_lossy())
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// ConfigRoot
// ---------------------------------------------------------------------------

/// Absolute directory that is a harness config root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ConfigRoot(AbsolutePath);

impl ConfigRoot {
    /// Create a validated config root.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let inner = AbsolutePath::new(value).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("ConfigRoot", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Create from a [`Path`].
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let inner = AbsolutePath::from_path(path).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("ConfigRoot", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Expand home vars at adapter boundary.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        let inner = AbsolutePath::expand_home(value, home).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("ConfigRoot", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Borrow as [`Path`].
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// Borrow inner [`AbsolutePath`].
    pub fn as_absolute(&self) -> &AbsolutePath {
        &self.0
    }

    /// Consume into inner.
    pub fn into_inner(self) -> AbsolutePath {
        self.0
    }

    /// Consume into [`PathBuf`].
    pub fn into_path_buf(self) -> PathBuf {
        self.0.into_inner()
    }
}

impl fmt::Display for ConfigRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<Path> for ConfigRoot {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl Deref for ConfigRoot {
    type Target = AbsolutePath;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Borrow<Path> for ConfigRoot {
    fn borrow(&self) -> &Path {
        self.0.as_path()
    }
}

impl FromStr for ConfigRoot {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for ConfigRoot {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl TryFrom<&str> for ConfigRoot {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ConfigRoot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// ConfigSurfacePath
// ---------------------------------------------------------------------------

/// Absolute path to a specific config surface (file) within a [`ConfigRoot`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ConfigSurfacePath(AbsolutePath);

impl ConfigSurfacePath {
    /// Create a validated surface path.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let inner = AbsolutePath::new(value).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("ConfigSurfacePath", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Create from a [`Path`].
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let inner = AbsolutePath::from_path(path).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("ConfigSurfacePath", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Expand home vars at adapter boundary.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        let inner = AbsolutePath::expand_home(value, home).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("ConfigSurfacePath", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Borrow as [`Path`].
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// Borrow inner.
    pub fn as_absolute(&self) -> &AbsolutePath {
        &self.0
    }

    /// Consume.
    pub fn into_inner(self) -> AbsolutePath {
        self.0
    }
}

impl fmt::Display for ConfigSurfacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<Path> for ConfigSurfacePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl Deref for ConfigSurfacePath {
    type Target = AbsolutePath;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Borrow<Path> for ConfigSurfacePath {
    fn borrow(&self) -> &Path {
        self.0.as_path()
    }
}

impl FromStr for ConfigSurfacePath {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for ConfigSurfacePath {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl TryFrom<&str> for ConfigSurfacePath {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ConfigSurfacePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// WrapperPath
// ---------------------------------------------------------------------------

/// Absolute path to a generated wrapper executable.
///
/// Symlink policy is handled by the mutation layer; this type does not
/// follow links.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct WrapperPath(AbsolutePath);

impl WrapperPath {
    /// Create a validated wrapper path.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let inner = AbsolutePath::new(value).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("WrapperPath", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Create from a [`Path`].
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let inner = AbsolutePath::from_path(path).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("WrapperPath", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Expand home vars at adapter boundary.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        let inner = AbsolutePath::expand_home(value, home).map_err(|e| match e {
            CoreError::InvalidPath { value, reason, .. } => {
                invalid_path("WrapperPath", &value, &reason)
            }
            other => other,
        })?;
        Ok(Self(inner))
    }

    /// Borrow as [`Path`].
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    /// Borrow inner.
    pub fn as_absolute(&self) -> &AbsolutePath {
        &self.0
    }

    /// Consume.
    pub fn into_inner(self) -> AbsolutePath {
        self.0
    }

    /// Consume into [`PathBuf`].
    pub fn into_path_buf(self) -> PathBuf {
        self.0.into_inner()
    }
}

impl fmt::Display for WrapperPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<Path> for WrapperPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl Deref for WrapperPath {
    type Target = AbsolutePath;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Borrow<Path> for WrapperPath {
    fn borrow(&self) -> &Path {
        self.0.as_path()
    }
}

impl FromStr for WrapperPath {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for WrapperPath {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl TryFrom<&str> for WrapperPath {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for WrapperPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// ExecutableRef
// ---------------------------------------------------------------------------

/// Reference to an executable, either a `PATH`-resolved name or an absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExecutableRef {
    /// Bare name resolved via `PATH`, e.g. `claude` or `code`.
    Named(String),
    /// Absolute filesystem path to the binary.
    Absolute(AbsolutePath),
}

impl ExecutableRef {
    /// Create from a string, validating as either name or absolute path.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        validate_not_empty("ExecutableRef", value)?;
        validate_no_nul("ExecutableRef", value)?;
        let path = Path::new(value);
        // If it looks like an absolute path, treat as absolute.
        if path.is_absolute() {
            // Validate as absolute path but map kind to ExecutableRef
            let abs = AbsolutePath::new(value).map_err(|e| match e {
                CoreError::InvalidPath { value, reason, .. } => {
                    invalid_path("ExecutableRef", &value, &reason)
                }
                other => other,
            })?;
            return Ok(Self::Absolute(abs));
        }
        // Otherwise must be a bare name: reject any path-like content
        if value.contains('/') || value.contains('\\') || value.contains(':') {
            return Err(invalid_path(
                "ExecutableRef",
                value,
                "executable name must not contain '/', '\\', or ':'",
            ));
        }
        if value == "." || value == ".." {
            return Err(invalid_path(
                "ExecutableRef",
                value,
                "must not be '.' or '..'",
            ));
        }
        // Reject traversal components even as name
        for comp in path.components() {
            if matches!(comp, Component::ParentDir | Component::CurDir) {
                return Err(invalid_path(
                    "ExecutableRef",
                    value,
                    "must not contain '.' or '..' components",
                ));
            }
        }
        // Also reject if name contains NUL already checked, and control chars
        if value.chars().any(|c| c == '\0' || c.is_control()) {
            return Err(invalid_path(
                "ExecutableRef",
                value,
                "must not contain control characters",
            ));
        }
        Ok(Self::Named(value.to_owned()))
    }

    /// Expand home vars at adapter boundary.
    ///
    /// If `value` starts with `~` or `$HOME`, it is expanded and treated as
    /// absolute. Otherwise the same rules as [`Self::new`] apply.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        validate_not_empty("ExecutableRef", value)?;
        if value.contains('\0') {
            return Err(invalid_path("ExecutableRef", value, "must not contain NUL"));
        }
        // If value starts with home var, expand and treat as absolute
        if value == "~"
            || value.starts_with("~/")
            || value.starts_with("~\\")
            || value.starts_with("$HOME/")
            || value.starts_with("${HOME}/")
            || value.starts_with("%USERPROFILE%/")
            || value.starts_with("%USERPROFILE%\\")
        {
            let expanded = expand_tilde(value, home);
            let display = expanded.to_string_lossy();
            let abs = AbsolutePath::from_path(&expanded).map_err(|e| match e {
                CoreError::InvalidPath { value, reason, .. } => {
                    invalid_path("ExecutableRef", &value, &reason)
                }
                other => other,
            })?;
            // Confirm expanded is absolute; if not, fall back to name handling
            // (but expand_tilde with home should produce absolute)
            let _ = display;
            return Ok(Self::Absolute(abs));
        }
        Self::new(value)
    }

    /// Returns true if this is an absolute path.
    pub fn is_absolute(&self) -> bool {
        matches!(self, Self::Absolute(_))
    }

    /// Returns true if this is a bare name.
    pub fn is_named(&self) -> bool {
        matches!(self, Self::Named(_))
    }

    /// Borrow as [`Path`] if absolute, else `None`.
    pub fn as_absolute_path(&self) -> Option<&AbsolutePath> {
        match self {
            Self::Absolute(p) => Some(p),
            Self::Named(_) => None,
        }
    }

    /// Borrow the name if named, else `None`.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Self::Named(s) => Some(s),
            Self::Absolute(_) => None,
        }
    }

    /// Display string for serialization.
    pub fn as_str(&self) -> String {
        match self {
            Self::Named(s) => s.clone(),
            Self::Absolute(p) => p.to_string_lossy().into_owned(),
        }
    }
}

impl fmt::Display for ExecutableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(s) => f.write_str(s),
            Self::Absolute(p) => write!(f, "{p}"),
        }
    }
}

impl FromStr for ExecutableRef {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for ExecutableRef {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl TryFrom<&str> for ExecutableRef {
    type Error = CoreError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl Serialize for ExecutableRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for ExecutableRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_valid() {
        let p = AbsolutePath::new("/home/user/.claude").unwrap();
        assert_eq!(p.as_path(), Path::new("/home/user/.claude"));
        let p2 = AbsolutePath::from_path(Path::new("/tmp/foo")).unwrap();
        assert_eq!(p2.as_path(), Path::new("/tmp/foo"));
        let p3: AbsolutePath = "/var/log".parse().unwrap();
        assert_eq!(p3.as_path(), Path::new("/var/log"));
        let p4 = AbsolutePath::try_from(String::from("/opt/bin")).unwrap();
        assert_eq!(p4.to_string(), "/opt/bin");
    }

    #[test]
    fn absolute_path_normalizes_dot_and_slash() {
        let p = AbsolutePath::new("/home//user/./.claude/").unwrap();
        // Normalized should not contain // or /./
        assert_eq!(p.as_path(), Path::new("/home/user/.claude"));
        let p2 = AbsolutePath::new("/a/b/./c").unwrap();
        assert_eq!(p2.as_path(), Path::new("/a/b/c"));
        // Root stays root
        let root = AbsolutePath::new("/").unwrap();
        assert_eq!(root.as_path(), Path::new("/"));
    }

    #[test]
    fn absolute_path_rejects_empty() {
        AbsolutePath::new("").unwrap_err();
        AbsolutePath::from_path(Path::new("")).unwrap_err();
    }

    #[test]
    fn absolute_path_rejects_nul() {
        AbsolutePath::new("/tmp/a\0b").unwrap_err();
        let path = Path::new("/tmp/a\0b");
        AbsolutePath::from_path(path).unwrap_err();
    }

    #[test]
    fn absolute_path_rejects_relative() {
        AbsolutePath::new("relative/path").unwrap_err();
        AbsolutePath::new("./relative").unwrap_err();
        AbsolutePath::new("a/b").unwrap_err();
        AbsolutePath::new("~/foo").unwrap_err();
    }

    #[test]
    fn absolute_path_rejects_traversal() {
        AbsolutePath::new("/home/../etc").unwrap_err();
        AbsolutePath::new("/a/b/../c").unwrap_err();
        AbsolutePath::new("/tmp/..").unwrap_err();
        AbsolutePath::new("/a/./../b").unwrap_err();
        // Even after normalization, traversal is rejected, not resolved
        let p = Path::new("/a/../b");
        AbsolutePath::from_path(p).unwrap_err();
    }

    #[test]
    fn absolute_path_expand_home_tilde() {
        let home = Path::new("/home/user");
        let p = AbsolutePath::expand_home("~/foo/bar", home).unwrap();
        assert_eq!(p.as_path(), Path::new("/home/user/foo/bar"));
        let p2 = AbsolutePath::expand_home("~", home).unwrap();
        assert_eq!(p2.as_path(), home);
        let p3 = AbsolutePath::expand_home("$HOME/.claude", home).unwrap();
        assert_eq!(p3.as_path(), Path::new("/home/user/.claude"));
        let p4 = AbsolutePath::expand_home("${HOME}/x", home).unwrap();
        assert_eq!(p4.as_path(), Path::new("/home/user/x"));
    }

    #[test]
    fn absolute_path_expand_home_rejects_traversal_after_expand() {
        let home = Path::new("/home/user");
        AbsolutePath::expand_home("~/../etc", home).unwrap_err();
        AbsolutePath::expand_home("~/a/../b", home).unwrap_err();
    }

    #[test]
    fn absolute_path_expand_home_rejects_nul_and_empty() {
        let home = Path::new("/home/user");
        AbsolutePath::expand_home("", home).unwrap_err();
        AbsolutePath::expand_home("/tmp/a\0b", home).unwrap_err();
        AbsolutePath::expand_home("~/a\0b", home).unwrap_err();
    }

    #[test]
    fn absolute_path_join() {
        let base = AbsolutePath::new("/home/user").unwrap();
        let joined = base.join("foo/bar").unwrap();
        assert_eq!(joined.as_path(), Path::new("/home/user/foo/bar"));
        base.join("../etc").unwrap_err();
        base.join("/absolute").unwrap_err();
        base.join("a\0b").unwrap_err();
        base.join("").unwrap_err();
    }

    #[test]
    fn absolute_path_does_not_follow_symlinks() {
        // No canonicalization: path is stored as given, not resolved
        let p = AbsolutePath::new("/tmp/link/to/file").unwrap();
        assert_eq!(p.as_path(), Path::new("/tmp/link/to/file"));
        // Even if symlink does not exist, we succeed (no canonicalize)
        let p2 = AbsolutePath::new("/nonexistent/path/to/file").unwrap();
        assert_eq!(p2.as_path(), Path::new("/nonexistent/path/to/file"));
    }

    #[test]
    fn absolute_path_serde_roundtrip() {
        let p = AbsolutePath::new("/home/user/.claude").unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"/home/user/.claude\"");
        let decoded: AbsolutePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, decoded);
        // Invalid deserialize
        let bad = "\"../etc\"";
        let res: Result<AbsolutePath, _> = serde_json::from_str(bad);
        res.unwrap_err();
        let bad2 = "\"/tmp/a\0b\"";
        let res: Result<AbsolutePath, _> = serde_json::from_str(bad2);
        res.unwrap_err();
    }

    #[test]
    fn config_root_wraps_absolute() {
        let r = ConfigRoot::new("/home/user/.claude").unwrap();
        assert_eq!(r.as_path(), Path::new("/home/user/.claude"));
        assert_eq!(r.to_string(), "/home/user/.claude");
        let r2 = ConfigRoot::expand_home("~/.claude", Path::new("/home/user")).unwrap();
        assert_eq!(r2.as_path(), Path::new("/home/user/.claude"));
        ConfigRoot::new("relative").unwrap_err();
        ConfigRoot::new("/a/../b").unwrap_err();
        ConfigRoot::new("/tmp/a\0b").unwrap_err();
        let json = serde_json::to_string(&r).unwrap();
        let decoded: ConfigRoot = serde_json::from_str(&json).unwrap();
        assert_eq!(r, decoded);
    }

    #[test]
    fn config_surface_path() {
        let s = ConfigSurfacePath::new("/home/user/.claude/settings.json").unwrap();
        assert_eq!(s.as_path(), Path::new("/home/user/.claude/settings.json"));
        let s2 = ConfigSurfacePath::expand_home("~/.claude/settings.json", Path::new("/home/user"))
            .unwrap();
        assert_eq!(s2.as_path(), Path::new("/home/user/.claude/settings.json"));
        ConfigSurfacePath::new("../relative").unwrap_err();
        ConfigSurfacePath::new("/a/../b").unwrap_err();
        let json = serde_json::to_string(&s).unwrap();
        let decoded: ConfigSurfacePath = serde_json::from_str(&json).unwrap();
        assert_eq!(s, decoded);
    }

    #[test]
    fn wrapper_path() {
        let w = WrapperPath::new("/usr/local/bin/work").unwrap();
        assert_eq!(w.as_path(), Path::new("/usr/local/bin/work"));
        let w2 = WrapperPath::expand_home("~/.local/bin/work", Path::new("/home/user")).unwrap();
        assert_eq!(w2.as_path(), Path::new("/home/user/.local/bin/work"));
        WrapperPath::new("relative/bin").unwrap_err();
        WrapperPath::new("/tmp/../etc").unwrap_err();
        let json = serde_json::to_string(&w).unwrap();
        let decoded: WrapperPath = serde_json::from_str(&json).unwrap();
        assert_eq!(w, decoded);
    }

    #[test]
    fn executable_ref_named() {
        let e = ExecutableRef::new("claude").unwrap();
        assert!(e.is_named());
        assert_eq!(e.as_name(), Some("claude"));
        assert!(!e.is_absolute());
        let e2 = ExecutableRef::new("code").unwrap();
        assert_eq!(e2.to_string(), "code");
        let e3: ExecutableRef = "python3".parse().unwrap();
        assert_eq!(e3.as_name(), Some("python3"));
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, "\"claude\"");
        let decoded: ExecutableRef = serde_json::from_str(&json).unwrap();
        assert_eq!(e, decoded);
    }

    #[test]
    fn executable_ref_absolute() {
        let e = ExecutableRef::new("/usr/bin/claude").unwrap();
        assert!(e.is_absolute());
        assert_eq!(
            e.as_absolute_path().unwrap().as_path(),
            Path::new("/usr/bin/claude")
        );
        let e2 = ExecutableRef::new("/opt/homebrew/bin/code").unwrap();
        assert!(e2.is_absolute());
        let json = serde_json::to_string(&e).unwrap();
        let decoded: ExecutableRef = serde_json::from_str(&json).unwrap();
        assert_eq!(e, decoded);
    }

    #[test]
    fn executable_ref_rejects_invalid() {
        ExecutableRef::new("").unwrap_err();
        ExecutableRef::new("a\0b").unwrap_err();
        ExecutableRef::new("a/b").unwrap_err();
        ExecutableRef::new("a\\b").unwrap_err();
        ExecutableRef::new("a:b").unwrap_err();
        ExecutableRef::new(".").unwrap_err();
        ExecutableRef::new("..").unwrap_err();
        ExecutableRef::new("foo/../bar").unwrap_err();
        ExecutableRef::new("/tmp/../etc/passwd").unwrap_err();
        ExecutableRef::new("/tmp/a\0b").unwrap_err();
        // Traversal in absolute
        ExecutableRef::new("/a/../b").unwrap_err();
    }

    #[test]
    fn executable_ref_expand_home() {
        let home = Path::new("/home/user");
        let e = ExecutableRef::expand_home("~/bin/claude", home).unwrap();
        assert!(e.is_absolute());
        assert_eq!(
            e.as_absolute_path().unwrap().as_path(),
            Path::new("/home/user/bin/claude")
        );
        let e2 = ExecutableRef::expand_home("claude", home).unwrap();
        assert!(e2.is_named());
        assert_eq!(e2.as_name(), Some("claude"));
        // Traversal after expand should fail
        ExecutableRef::expand_home("~/../etc", home).unwrap_err();
        ExecutableRef::expand_home("", home).unwrap_err();
    }

    #[test]
    fn paths_preserve_symlink_semantics() {
        // Path types alone do not resolve symlinks; they store the lexical path.
        let p = AbsolutePath::new("/tmp/mylink").unwrap();
        // No filesystem check, so this succeeds even if mylink is a symlink
        // or does not exist.
        assert_eq!(p.as_path(), Path::new("/tmp/mylink"));
        let w = WrapperPath::new("/usr/local/bin/my-wrapper").unwrap();
        assert_eq!(w.as_path(), Path::new("/usr/local/bin/my-wrapper"));
    }

    #[test]
    fn all_path_types_reject_nul_and_empty_and_traversal() {
        let cases = ["", "/tmp/a\0b", "/a/../b", "relative/path"];
        for c in cases {
            AbsolutePath::new(c).unwrap_err();
            ConfigRoot::new(c).unwrap_err();
            ConfigSurfacePath::new(c).unwrap_err();
            WrapperPath::new(c).unwrap_err();
        }
        // ExecutableRef name cases
        ExecutableRef::new("").unwrap_err();
        ExecutableRef::new("a\0b").unwrap_err();
        ExecutableRef::new("a/b").unwrap_err();
        ExecutableRef::new("/a/../b").unwrap_err();
    }

    #[test]
    fn executable_ref_serde() {
        let named = ExecutableRef::new("claude").unwrap();
        let json = serde_json::to_string(&named).unwrap();
        let back: ExecutableRef = serde_json::from_str(&json).unwrap();
        assert_eq!(named, back);

        let abs = ExecutableRef::new("/usr/local/bin/claude").unwrap();
        let json = serde_json::to_string(&abs).unwrap();
        let back: ExecutableRef = serde_json::from_str(&json).unwrap();
        assert_eq!(abs, back);

        // Invalid deserialize
        let bad = "\"a/b\"";
        let res: Result<ExecutableRef, _> = serde_json::from_str(bad);
        res.unwrap_err();
    }

    #[test]
    fn display_and_from_str() {
        let p: AbsolutePath = "/tmp/foo".parse().unwrap();
        assert_eq!(format!("{p}"), "/tmp/foo");
        let c: ConfigRoot = "/tmp/root".parse().unwrap();
        assert_eq!(format!("{c}"), "/tmp/root");
        let w: WrapperPath = "/tmp/wrapper".parse().unwrap();
        assert_eq!(format!("{w}"), "/tmp/wrapper");
        let e: ExecutableRef = "mybin".parse().unwrap();
        assert_eq!(format!("{e}"), "mybin");
        let e2: ExecutableRef = "/usr/bin/mybin".parse().unwrap();
        assert_eq!(format!("{e2}"), "/usr/bin/mybin");
    }
}
