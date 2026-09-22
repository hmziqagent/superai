//! Path and executable reference types: normalized absolute forms, no
//! symlink following, `..` always rejected; home expansion at the boundary.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::CoreError;

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
    Ok(normalize_absolute(path))
}

/// Relabel an [`AbsolutePath`] error so it names the wrapping type.
fn rekind(kind: &str, result: Result<AbsolutePath, CoreError>) -> Result<AbsolutePath, CoreError> {
    result.map_err(|e| match e {
        CoreError::InvalidPath { value, reason, .. } => invalid_path(kind, &value, &reason),
        other => other,
    })
}

/// Normalized absolute path, no symlink following: rejects empty, NUL,
/// non-absolute, and `..` components; normalization is lexical only.
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

    /// Expand a leading `~` or platform home variable via `home`, then
    /// validate. This is the adapter boundary: raw strings expand only here.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        validate_not_empty("AbsolutePath", value)?;
        if value.contains('\0') {
            return Err(invalid_path("AbsolutePath", value, "must not contain NUL"));
        }
        let expanded = expand_tilde(value, home);
        let display = expanded.to_string_lossy();
        let normalized = validate_absolute_path("AbsolutePath", &expanded, display.as_ref())?;
        Ok(Self(normalized))
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
        Ok(Self(normalize_absolute(&self.0.join(rel_path))))
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
        // Non-UTF-8 paths yield an empty string; use `as_path` for those.
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

/// Absolute directory that is a harness config root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ConfigRoot(AbsolutePath);

impl ConfigRoot {
    /// Create a validated config root.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let inner = rekind("ConfigRoot", AbsolutePath::new(value))?;
        Ok(Self(inner))
    }

    /// Create from a [`Path`].
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let inner = rekind("ConfigRoot", AbsolutePath::from_path(path))?;
        Ok(Self(inner))
    }

    /// Expand home vars at adapter boundary.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        let inner = rekind("ConfigRoot", AbsolutePath::expand_home(value, home))?;
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

/// Absolute path to a specific config surface (file) within a [`ConfigRoot`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ConfigSurfacePath(AbsolutePath);

impl ConfigSurfacePath {
    /// Create a validated surface path.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let inner = rekind("ConfigSurfacePath", AbsolutePath::new(value))?;
        Ok(Self(inner))
    }

    /// Create from a [`Path`].
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let inner = rekind("ConfigSurfacePath", AbsolutePath::from_path(path))?;
        Ok(Self(inner))
    }

    /// Expand home vars at adapter boundary.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        let inner = rekind("ConfigSurfacePath", AbsolutePath::expand_home(value, home))?;
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

/// Absolute path to a generated wrapper executable; does not follow links,
/// symlink policy lives in the mutation layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct WrapperPath(AbsolutePath);

impl WrapperPath {
    /// Create a validated wrapper path.
    pub fn new(value: &str) -> Result<Self, CoreError> {
        let inner = rekind("WrapperPath", AbsolutePath::new(value))?;
        Ok(Self(inner))
    }

    /// Create from a [`Path`].
    pub fn from_path(path: &Path) -> Result<Self, CoreError> {
        let inner = rekind("WrapperPath", AbsolutePath::from_path(path))?;
        Ok(Self(inner))
    }

    /// Expand home vars at adapter boundary.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        let inner = rekind("WrapperPath", AbsolutePath::expand_home(value, home))?;
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
        if path.is_absolute() {
            let abs = rekind("ExecutableRef", AbsolutePath::new(value))?;
            return Ok(Self::Absolute(abs));
        }
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
        for comp in path.components() {
            if matches!(comp, Component::ParentDir | Component::CurDir) {
                return Err(invalid_path(
                    "ExecutableRef",
                    value,
                    "must not contain '.' or '..' components",
                ));
            }
        }
        if value.chars().any(char::is_control) {
            return Err(invalid_path(
                "ExecutableRef",
                value,
                "must not contain control characters",
            ));
        }
        Ok(Self::Named(value.to_owned()))
    }

    /// Expand home vars at adapter boundary; a `~`/`$HOME` prefix becomes
    /// absolute, otherwise the rules of [`Self::new`] apply.
    pub fn expand_home(value: &str, home: &Path) -> Result<Self, CoreError> {
        validate_not_empty("ExecutableRef", value)?;
        if value.contains('\0') {
            return Err(invalid_path("ExecutableRef", value, "must not contain NUL"));
        }
        if value == "~"
            || value.starts_with("~/")
            || value.starts_with("~\\")
            || value.starts_with("$HOME/")
            || value.starts_with("${HOME}/")
            || value.starts_with("%USERPROFILE%/")
            || value.starts_with("%USERPROFILE%\\")
        {
            let expanded = expand_tilde(value, home);
            let abs = rekind("ExecutableRef", AbsolutePath::from_path(&expanded))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `/a/b` is not absolute on Windows (no drive), so tests needing an
    /// absolute literal must go through this helper.
    fn abs(s: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{}", s.replace('/', "\\"))
        } else {
            format!("/{s}")
        }
    }

    #[test]
    fn absolute_path_valid() {
        let home_like = crate::test_util::tmp_abs_str("user/.claude");
        let p = AbsolutePath::new(&home_like).unwrap();
        assert_eq!(p.as_path(), Path::new(&home_like));
        let tmp_like = crate::test_util::tmp_abs_str("foo");
        let p2 = AbsolutePath::from_path(Path::new(&tmp_like)).unwrap();
        assert_eq!(p2.as_path(), Path::new(&tmp_like));
        let var_like = crate::test_util::tmp_abs_str("log");
        let p3: AbsolutePath = var_like.parse().unwrap();
        assert_eq!(p3.as_path(), Path::new(&var_like));
        let opt_like = crate::test_util::tmp_abs_str("bin");
        let p4 = AbsolutePath::try_from(opt_like.clone()).unwrap();
        assert_eq!(p4.to_string(), opt_like);
    }

    #[test]
    fn absolute_path_normalizes_dot_and_slash() {
        let base = crate::test_util::tmp_abs_str("home/user");
        let p = AbsolutePath::new(&format!("{base}//.claude/./x/")).unwrap();
        assert_eq!(p.as_path(), Path::new(&format!("{base}/.claude/x")));
        let p2 = AbsolutePath::new(&format!("{base}/b/./c")).unwrap();
        assert_eq!(p2.as_path(), Path::new(&format!("{base}/b/c")));
        let root_str = if cfg!(windows) { "C:\\" } else { "/" };
        let root = AbsolutePath::new(root_str).unwrap();
        assert_eq!(root.as_path(), Path::new(root_str));
    }

    #[test]
    fn absolute_path_rejects_empty() {
        AbsolutePath::new("").unwrap_err();
        AbsolutePath::from_path(Path::new("")).unwrap_err();
    }

    #[test]
    fn absolute_path_rejects_nul() {
        let with_nul = format!("{}\0b", crate::test_util::tmp_abs_str("nul-a"));
        AbsolutePath::new(&with_nul).unwrap_err();
        let path = Path::new(&with_nul);
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
        AbsolutePath::new(&format!(
            "{}/../etc",
            crate::test_util::tmp_abs_str("trav-home")
        ))
        .unwrap_err();
        AbsolutePath::new("/a/b/../c").unwrap_err();
        AbsolutePath::new(&format!("{}/..", crate::test_util::tmp_abs_str("dotdot"))).unwrap_err();
        AbsolutePath::new("/a/./../b").unwrap_err();
        // Even after normalization, traversal is rejected, not resolved
        let p = Path::new("/a/../b");
        AbsolutePath::from_path(p).unwrap_err();
    }

    #[test]
    fn absolute_path_expand_home_tilde() {
        let home = crate::test_util::tmp_abs("user");
        let p = AbsolutePath::expand_home("~/foo/bar", &home).unwrap();
        assert_eq!(p.as_path(), home.join("foo/bar"));
        let p2 = AbsolutePath::expand_home("~", &home).unwrap();
        assert_eq!(p2.as_path(), &home);
        let p3 = AbsolutePath::expand_home("$HOME/.claude", &home).unwrap();
        assert_eq!(p3.as_path(), home.join(".claude"));
        let p4 = AbsolutePath::expand_home("${HOME}/x", &home).unwrap();
        assert_eq!(p4.as_path(), home.join("x"));
    }

    #[test]
    fn absolute_path_expand_home_rejects_traversal_after_expand() {
        let home = crate::test_util::tmp_abs("user");
        AbsolutePath::expand_home("~/../etc", &home).unwrap_err();
        AbsolutePath::expand_home("~/a/../b", &home).unwrap_err();
    }

    #[test]
    fn absolute_path_expand_home_rejects_nul_and_empty() {
        let home = crate::test_util::tmp_abs("user");
        AbsolutePath::expand_home("", &home).unwrap_err();
        AbsolutePath::expand_home(
            &format!("{}\0b", crate::test_util::tmp_abs_str("nul-b")),
            &home,
        )
        .unwrap_err();
        AbsolutePath::expand_home("~/a\0b", &home).unwrap_err();
    }

    #[test]
    fn absolute_path_join() {
        let base = AbsolutePath::from_path(&crate::test_util::tmp_abs("user")).unwrap();
        let joined = base.join("foo/bar").unwrap();
        assert_eq!(joined.as_path(), base.join("foo/bar").unwrap().as_path());
        base.join("../etc").unwrap_err();
        base.join(&abs("absolute")).unwrap_err();
        base.join("a\0b").unwrap_err();
        base.join("").unwrap_err();
    }

    #[test]
    fn absolute_path_does_not_follow_symlinks() {
        let link_path = crate::test_util::tmp_abs_str("link/to/file");
        let p = AbsolutePath::new(&link_path).unwrap();
        assert_eq!(p.as_path(), Path::new(&link_path));
        // No canonicalize: construction succeeds even when parents do not exist.
        let nonexistent = crate::test_util::tmp_abs_str("nonexistent-parent") + "/path/to/file";
        let p2 = AbsolutePath::new(&nonexistent).unwrap();
        assert_eq!(p2.as_path(), Path::new(&nonexistent));
    }

    #[test]
    fn absolute_path_serde_roundtrip() {
        let fixture = crate::test_util::tmp_abs_str("user/.claude");
        let p = AbsolutePath::new(&fixture).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        // The serialized form is the JSON encoding of the fixture string
        // (backslashes escaped on Windows), not the raw fixture.
        assert_eq!(json, serde_json::to_string(&fixture).unwrap());
        let decoded: AbsolutePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, decoded);
        let bad = "\"../etc\"";
        let res: Result<AbsolutePath, _> = serde_json::from_str(bad);
        res.unwrap_err();
        let bad2 = format!("\"{}\0b\"", crate::test_util::tmp_abs_str("nul-c"));
        let res: Result<AbsolutePath, _> = serde_json::from_str(&bad2);
        res.unwrap_err();
    }

    #[test]
    fn config_root_wraps_absolute() {
        let fixture = crate::test_util::tmp_abs_str("user/.claude");
        let r = ConfigRoot::new(&fixture).unwrap();
        assert_eq!(r.as_path(), Path::new(&fixture));
        assert_eq!(r.to_string(), fixture);
        let home = crate::test_util::tmp_abs("user");
        let r2 = ConfigRoot::expand_home("~/.claude", &home).unwrap();
        assert_eq!(r2.as_path(), home.join(".claude"));
        ConfigRoot::new("relative").unwrap_err();
        ConfigRoot::new("/a/../b").unwrap_err();
        ConfigRoot::new(&format!("{}\0b", crate::test_util::tmp_abs_str("nul-d"))).unwrap_err();
        let json = serde_json::to_string(&r).unwrap();
        let decoded: ConfigRoot = serde_json::from_str(&json).unwrap();
        assert_eq!(r, decoded);
    }

    #[test]
    fn config_surface_path() {
        let fixture = crate::test_util::tmp_abs_str("user/.claude/settings.json");
        let s = ConfigSurfacePath::new(&fixture).unwrap();
        assert_eq!(s.as_path(), Path::new(&fixture));
        let home = crate::test_util::tmp_abs("user");
        let s2 = ConfigSurfacePath::expand_home("~/.claude/settings.json", &home).unwrap();
        assert_eq!(s2.as_path(), home.join(".claude").join("settings.json"));
        ConfigSurfacePath::new("../relative").unwrap_err();
        ConfigSurfacePath::new("/a/../b").unwrap_err();
        let json = serde_json::to_string(&s).unwrap();
        let decoded: ConfigSurfacePath = serde_json::from_str(&json).unwrap();
        assert_eq!(s, decoded);
    }

    #[test]
    fn wrapper_path() {
        let fixture = crate::test_util::tmp_abs_str("local/bin/work");
        let w = WrapperPath::new(&fixture).unwrap();
        assert_eq!(w.as_path(), Path::new(&fixture));
        let home = crate::test_util::tmp_abs("user");
        let w2 = WrapperPath::expand_home("~/.local/bin/work", &home).unwrap();
        assert_eq!(w2.as_path(), home.join(".local").join("bin").join("work"));
        WrapperPath::new("relative/bin").unwrap_err();
        WrapperPath::new(&format!(
            "{}/../etc",
            crate::test_util::tmp_abs_str("trav-w")
        ))
        .unwrap_err();
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
        let fixture = crate::test_util::tmp_abs_str("usr/bin/claude");
        let e = ExecutableRef::new(&fixture).unwrap();
        assert!(e.is_absolute());
        assert_eq!(e.as_absolute_path().unwrap().as_path(), Path::new(&fixture));
        let fixture2 = crate::test_util::tmp_abs_str("opt/homebrew/bin/code");
        let e2 = ExecutableRef::new(&fixture2).unwrap();
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
        ExecutableRef::new(&format!(
            "{}/../etc/passwd",
            crate::test_util::tmp_abs_str("trav-e")
        ))
        .unwrap_err();
        ExecutableRef::new(&format!("{}\0b", crate::test_util::tmp_abs_str("nul-e"))).unwrap_err();
        ExecutableRef::new("/a/../b").unwrap_err();
    }

    #[test]
    fn executable_ref_expand_home() {
        let home = crate::test_util::tmp_abs("user");
        let e = ExecutableRef::expand_home("~/bin/claude", &home).unwrap();
        assert!(e.is_absolute());
        assert_eq!(
            e.as_absolute_path().unwrap().as_path(),
            home.join("bin").join("claude")
        );
        let e2 = ExecutableRef::expand_home("claude", &home).unwrap();
        assert!(e2.is_named());
        assert_eq!(e2.as_name(), Some("claude"));
        ExecutableRef::expand_home("~/../etc", &home).unwrap_err();
        ExecutableRef::expand_home("", &home).unwrap_err();
    }

    #[test]
    fn paths_preserve_symlink_semantics() {
        // Path types alone do not resolve symlinks; they store the lexical path.
        let link = crate::test_util::tmp_abs_str("mylink");
        let p = AbsolutePath::new(&link).unwrap();
        assert_eq!(p.as_path(), Path::new(&link));
        let wrapper = crate::test_util::tmp_abs_str("local/bin/my-wrapper");
        let w = WrapperPath::new(&wrapper).unwrap();
        assert_eq!(w.as_path(), Path::new(&wrapper));
    }

    #[test]
    fn all_path_types_reject_nul_and_empty_and_traversal() {
        let nul_abs = format!("{}\0b", crate::test_util::tmp_abs_str("nul-f"));
        let cases: [&str; 4] = ["", &nul_abs, "/a/../b", "relative/path"];
        for c in cases {
            AbsolutePath::new(c).unwrap_err();
            ConfigRoot::new(c).unwrap_err();
            ConfigSurfacePath::new(c).unwrap_err();
            WrapperPath::new(c).unwrap_err();
        }
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

        let abs = ExecutableRef::new(&crate::test_util::tmp_abs_str("local/bin/claude")).unwrap();
        let json = serde_json::to_string(&abs).unwrap();
        let back: ExecutableRef = serde_json::from_str(&json).unwrap();
        assert_eq!(abs, back);

        let bad = "\"a/b\"";
        let res: Result<ExecutableRef, _> = serde_json::from_str(bad);
        res.unwrap_err();
    }

    #[test]
    fn display_and_from_str() {
        let foo = crate::test_util::tmp_abs_str("foo");
        let p: AbsolutePath = foo.parse().unwrap();
        assert_eq!(format!("{p}"), foo);
        let root = crate::test_util::tmp_abs_str("root");
        let c: ConfigRoot = root.parse().unwrap();
        assert_eq!(format!("{c}"), root);
        let wrapper = crate::test_util::tmp_abs_str("wrapper");
        let w: WrapperPath = wrapper.parse().unwrap();
        assert_eq!(format!("{w}"), wrapper);
        let e: ExecutableRef = "mybin".parse().unwrap();
        assert_eq!(format!("{e}"), "mybin");
        let mybin = crate::test_util::tmp_abs_str("bin/mybin");
        let e2: ExecutableRef = mybin.parse().unwrap();
        assert_eq!(format!("{e2}"), mybin);
    }
}
