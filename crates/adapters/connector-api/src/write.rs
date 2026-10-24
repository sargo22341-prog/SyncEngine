//! Écriture en deux temps : contenu déposé dans une zone temporaire, puis
//! validation (« commit ») explicite.

use sync_core::{FileEntry, ResourcePath, Timestamp};

use crate::error::StorageError;

/// Options d'ouverture d'une écriture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WriteOptions {
    /// Date de modification à appliquer au fichier validé (en général celle
    /// de la source), afin qu'une comparaison ultérieure ne le voie pas
    /// « plus récent » que la source. `None` : date choisie par le stockage.
    /// Le connecteur doit déclarer `StorageFeature::SetModifiedTime` pour accepter
    /// une valeur ; `sync-copy` écarte cette option lorsque la capacité manque.
    pub modified: Option<Timestamp>,
    /// Taille attendue, indicative (préallocation, contrôle de quota).
    pub size_hint: Option<u64>,
}

impl WriteOptions {
    /// Aucune option.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Date de modification à appliquer au fichier validé.
    #[must_use]
    pub fn with_modified(mut self, modified: Option<Timestamp>) -> Self {
        self.modified = modified;
        self
    }

    /// Taille attendue, indicative.
    #[must_use]
    pub fn with_size_hint(mut self, size: Option<u64>) -> Self {
        self.size_hint = size;
        self
    }
}

/// Sémantique de la validation d'une écriture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CommitMode {
    /// La cible doit être absente : si elle existe, la validation échoue
    /// avec [`StorageErrorKind::Conflict`](crate::StorageErrorKind::Conflict)
    /// et la cible n'est pas modifiée.
    CreateNew,
    /// La cible existante (fichier ordinaire) est remplacée. Le connecteur
    /// documente si ce remplacement est atomique ; l'appelant revérifie
    /// lui-même l'état de la cible juste avant de valider.
    ReplaceExisting,
}

/// Écriture en cours d'un fichier.
///
/// # Contrat
/// - le contenu est d'abord déposé dans une **zone temporaire** : la cible
///   n'est ni créée ni modifiée avant [`commit`](Self::commit) ;
/// - [`abort`](Self::abort) (ou l'abandon de l'objet sans validation) retire
///   le dépôt temporaire ; l'abandon implicite est « au mieux » et ne peut
///   pas signaler d'échec, l'appelant préfère donc `abort` ;
/// - après une erreur de [`write`](Self::write), seul `abort` a un sens ;
/// - l'implémentation consulte le jeton d'annulation reçu à l'ouverture.
pub trait ContentWriter: Send {
    /// Écrit tout `data` à la suite du contenu déjà déposé.
    ///
    /// # Errors
    /// [`InsufficientSpace`](crate::StorageErrorKind::InsufficientSpace)
    /// (disque plein, quota), [`Disconnected`](crate::StorageErrorKind::Disconnected),
    /// [`Cancelled`](crate::StorageErrorKind::Cancelled)…
    fn write(&mut self, data: &[u8]) -> Result<(), StorageError>;

    /// Chemin logique du dépôt temporaire, s'il en a un dans l'arborescence
    /// de l'extrémité (pour le relire avant validation et le consigner au
    /// journal, afin qu'une exécution interrompue sache ce qu'elle a laissé).
    fn staging_path(&self) -> Option<&ResourcePath>;

    /// Termine le dépôt sans publier la cible, afin de permettre sa relecture.
    /// Après succès, `write` ne doit plus être appelé. Idempotent ; les anciens
    /// connecteurs dont le dépôt est déjà lisible conservent le défaut sans effet.
    ///
    /// # Errors
    /// Échec de finalisation/transfert, annulation ou quota. Seul `abort` suit
    /// une erreur ; aucune publication de la cible n'est autorisée.
    fn finish(&mut self) -> Result<(), StorageError> {
        Ok(())
    }

    /// Relit depuis le début, après [`finish`](Self::finish), le contenu
    /// déposé, par l'accès que ce writer détient déjà (sans rouvrir le dépôt
    /// par son chemin), en passant chaque bloc lu à `sink`. Sert à la
    /// vérification avant publication ; la cible reste intacte.
    ///
    /// `Ok(false)` : relecture directe non prise en charge (défaut) ; l'appelant
    /// relit alors [`staging_path`](Self::staging_path) par `open_read`.
    ///
    /// # Errors
    /// Lecture impossible ou annulation ; seul `abort` suit une erreur.
    fn read_back(
        &mut self,
        _buffer: &mut [u8],
        _sink: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, StorageError> {
        Ok(false)
    }

    /// Rend le contenu durable, puis le publie à la cible selon `mode`.
    ///
    /// En cas d'échec, le dépôt temporaire est retiré au mieux et la cible
    /// est inchangée, sauf mention contraire du connecteur.
    ///
    /// # Errors
    /// [`Conflict`](crate::StorageErrorKind::Conflict) si `mode` vaut
    /// [`CommitMode::CreateNew`] et que la cible existe ; autres catégories
    /// selon la cause.
    fn commit(self: Box<Self>, mode: CommitMode) -> Result<FileEntry, StorageError>;

    /// Abandonne l'écriture et retire le dépôt temporaire.
    ///
    /// # Errors
    /// Le dépôt temporaire n'a pas pu être retiré (il est alors signalé par
    /// [`staging_path`](Self::staging_path), consigné avant l'erreur).
    fn abort(self: Box<Self>) -> Result<(), StorageError>;
}
