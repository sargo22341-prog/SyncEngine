//! Propriétés du volume qui porte la racine (Linux : `statfs`).
//!
//! La précision des dates dépend du système de fichiers, pas de l'OS : FAT
//! garde 2 s, exFAT 10 ms, un montage FUSE ou un partage réseau dépend du
//! serveur. Une précision annoncée trop fine fait voir « plus récents » des
//! fichiers identiques copiés sur une clé USB, à chaque passage (audit
//! Linux M9). Hors Linux, rien n'est détecté ici (Windows :
//! `GetVolumeInformationW`, à faire et à vérifier sous Windows).
//!
//! Limite : seule la racine est examinée ; un autre volume monté plus bas
//! dans l'arborescence garde la précision de la racine.

use sync_core::TimestampPrecision;

/// Types `statfs` (`f_type`) reconnus.
#[cfg(any(target_os = "linux", test))]
const MSDOS_SUPER_MAGIC: u64 = 0x4d44;
#[cfg(any(target_os = "linux", test))]
const EXFAT_SUPER_MAGIC: u64 = 0x2011_bab0;
#[cfg(any(target_os = "linux", test))]
const FUSE_SUPER_MAGIC: u64 = 0x6573_5546;
#[cfg(any(target_os = "linux", test))]
const NTFS3_SUPER_MAGIC: u64 = 0x7366_746e;
#[cfg(any(target_os = "linux", test))]
const NTFS_SB_MAGIC: u64 = 0x5346_544e;
#[cfg(any(target_os = "linux", test))]
const SMB_SUPER_MAGIC: u64 = 0x517b;
#[cfg(any(target_os = "linux", test))]
const CIFS_SUPER_MAGIC: u64 = 0xff53_4d42;
#[cfg(any(target_os = "linux", test))]
const SMB2_SUPER_MAGIC: u64 = 0xfe53_4d42;

/// Précision des dates garantie par un type de système de fichiers ;
/// `None` : celle de l'OS (nanoseconde sous Linux).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn precision_for(fs_type: u64) -> Option<TimestampPrecision> {
    match fs_type {
        // vfat / msdos : 2 s pour la date de modification.
        MSDOS_SUPER_MAGIC => Some(TimestampPrecision::TwoSeconds),
        // exFAT : 10 ms ; la seconde est la précision représentable sûre.
        EXFAT_SUPER_MAGIC => Some(TimestampPrecision::Second),
        // FUSE (exfat-fuse : seconde ; ntfs-3g plus fin) : serveur inconnu.
        FUSE_SUPER_MAGIC => Some(TimestampPrecision::Second),
        NTFS3_SUPER_MAGIC | NTFS_SB_MAGIC => Some(TimestampPrecision::HundredNanoseconds),
        // Partage SMB/CIFS : le volume du serveur peut être FAT.
        SMB_SUPER_MAGIC | CIFS_SUPER_MAGIC | SMB2_SUPER_MAGIC => {
            Some(TimestampPrecision::TwoSeconds)
        }
        _ => None,
    }
}

/// Précision détectée pour le volume de `root`, si elle diffère de celle de
/// l'OS. Un `statfs` impossible (racine absente) ne détecte rien.
pub(crate) fn detected_precision(root: &std::path::Path) -> Option<TimestampPrecision> {
    #[cfg(target_os = "linux")]
    {
        let stats = rustix::fs::statfs(root).ok()?;
        // Magic numbers fit in 32 bits; `f_type` is signed on some targets.
        let fs_type = u64::from(stats.f_type as u32);
        precision_for(fs_type)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removable_and_network_volumes_get_coarse_dates() {
        assert_eq!(
            precision_for(MSDOS_SUPER_MAGIC),
            Some(TimestampPrecision::TwoSeconds)
        );
        assert_eq!(
            precision_for(EXFAT_SUPER_MAGIC),
            Some(TimestampPrecision::Second)
        );
        assert_eq!(
            precision_for(FUSE_SUPER_MAGIC),
            Some(TimestampPrecision::Second)
        );
        assert_eq!(
            precision_for(CIFS_SUPER_MAGIC),
            Some(TimestampPrecision::TwoSeconds)
        );
        // ext4, xfs, btrfs, tmpfs : précision de l'OS.
        for native in [0xef53_u64, 0x5846_5342, 0x9123_683e, 0x0102_1994] {
            assert_eq!(precision_for(native), None);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_native_temporary_folder_keeps_the_os_precision() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        // Selon le conteneur : ext4, overlay ou tmpfs, jamais FAT.
        assert_ne!(
            detected_precision(dir.path()),
            Some(TimestampPrecision::TwoSeconds)
        );
        Ok(())
    }
}
