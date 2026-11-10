//! Windows paths, made to look like the Git Bash paths Claude Code's commands use
//! there (`C:\Users\me` is `/c/Users/me`), so the path checks elsewhere, written
//! for `/`-rooted paths, work unchanged. Any other string is returned as it is.

pub fn slash(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'\\' || b[2] == b'/') {
        let rest = p[2..].replace('\\', "/");
        return format!("/{}{}", (b[0] as char).to_ascii_lowercase(), rest.trim_end_matches('/'));
    }
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::slash;

    #[test]
    fn a_drive_path_becomes_a_git_bash_path() {
        assert_eq!(slash(r"C:\Users\me\dev\app"), "/c/Users/me/dev/app");
        assert_eq!(slash("D:/work/notes.md"), "/d/work/notes.md");
        assert_eq!(slash(r"C:\"), "/c");
        assert_eq!(slash("C:"), "/c");
    }

    #[test]
    fn anything_else_is_left_alone() {
        for p in ["/Users/me/dev/app", "~/HarnessAgents", "src/a.rs", "", "a:b", r"x\y", "C:foo"] {
            assert_eq!(slash(p), p);
        }
    }
}
