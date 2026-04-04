macro_rules! dep {
    ($name:ident, $var:literal, $fallback:literal) => {
        #[cfg(nix)]
        pub const $name: &str = env!($var, concat!("missing ", $var, " in nix build"));
        #[cfg(not(nix))]
        pub const $name: &str = $fallback;
    };
}

dep!(BASH, "VICODE_BASH", "bash");
dep!(BINDFS, "VICODE_BINDFS", "bindfs");
dep!(BWRAP, "VICODE_BWRAP", "bwrap");
dep!(FUSE_OVERLAYFS, "VICODE_FUSE_OVERLAYFS", "fuse-overlayfs");
dep!(GIT, "VICODE_GIT", "git");
dep!(MOUNTPOINT, "VICODE_MOUNTPOINT", "mountpoint");
dep!(TAR, "VICODE_TAR", "tar");

pub const UMOUNT: &str = "umount";
