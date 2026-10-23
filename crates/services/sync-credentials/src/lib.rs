//! Port d'authentification runtime. Aucun login OAuth, stockage OS ou refresh
//! token n'est implémenté ici. L'application fournit un service externe borné.
use sync_core::CancellationToken;

/// Erreur sans message fournisseur susceptible de contenir un secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    /// Jeton absent, expiré, révoqué ou syntaxiquement invalide.
    Unavailable,
    /// Travail annulé.
    Cancelled,
}
impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "identifiants indisponibles",
            Self::Cancelled => "authentification annulée",
        })
    }
}
impl std::error::Error for CredentialError {}

/// Jeton d'accès éphémère, sans Debug ni Serialize. Pas d'effacement mémoire garanti.
pub struct AccessToken(String);
impl AccessToken {
    /// Valide la grammaire b64token de RFC 6750, sans recopier l'entrée en erreur.
    /// # Errors
    /// `Unavailable` si vide ou invalide.
    pub fn new(value: String) -> Result<Self, CredentialError> {
        let stem = value.trim_end_matches('=');
        if stem.is_empty()
            || !stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b))
        {
            return Err(CredentialError::Unavailable);
        }
        Ok(Self(value))
    }
    /// Exposition explicite au transport, jamais aux diagnostics ou profils.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Service externe lié à une audience et un compte choisis par l'application.
/// L'implémentation borne ses délais, gère expiration/renouvellement/révocation,
/// vérifie les scopes et ne persiste pas de secret dans un profil.
pub trait AccessTokenProvider: Send + Sync {
    /// Fournit un jeton valide pour la requête suivante. Aucun retry de login
    /// implicite après 401 : le connecteur remonte Unauthenticated.
    /// # Errors
    /// `Unavailable` ou `Cancelled`, sans détail secret.
    fn access_token(&self, cancel: &CancellationToken) -> Result<AccessToken, CredentialError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_header_injection_without_echo() {
        for value in ["", "=", "a b", "a\r\nsecret", "a=b"] {
            assert_eq!(
                AccessToken::new(value.into()).err(),
                Some(CredentialError::Unavailable)
            );
        }
        assert!(AccessToken::new("abc.def-_/+==".into()).is_ok());
    }
}
