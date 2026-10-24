use crate::Error;
use serde::Deserialize;
use url::Url;

/// Progress from the current registration attempt; never contains a bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollmentProgress {
    VerificationRequired(String),
    Applying,
}

#[derive(Deserialize)]
pub(crate) struct Enrollment {
    pub request_id: String,
    pub site_id: String,
    pub public_key_fingerprint: String,
    pub state: EnrollmentState,
    #[serde(default)]
    pub verification_url: Option<String>,
    #[serde(default)]
    pub reason: Option<EnrollmentReason>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EnrollmentState {
    Pending,
    Applied,
    Failed,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EnrollmentReason {
    VerificationRequired,
    KeyRevoked,
    KeyExpired,
    SyncFailed,
    ApprovalExpired,
}

pub(crate) enum Status {
    Applied,
    Pending(Option<String>),
}

impl Enrollment {
    pub fn validate(
        &self,
        request_id: &str,
        site_id: &str,
        fingerprint: &str,
        portal: &Url,
        browser_verification: bool,
    ) -> Result<Status, Error> {
        if self.request_id != request_id
            || self.site_id != site_id
            || self.public_key_fingerprint != fingerprint
        {
            return Err(Error::Contract);
        }
        match self.state {
            EnrollmentState::Pending => {
                if !matches!(
                    self.reason,
                    None | Some(EnrollmentReason::VerificationRequired)
                ) {
                    return Err(Error::Contract);
                }
                match &self.verification_url {
                    Some(value) => {
                        let expected = portal
                            .join(&format!(
                                "/portal/#/portal/ssh-device?request_id={request_id}"
                            ))
                            .map_err(|_| Error::Contract)?;
                        // Exact approved route and request binding. Do not accept extra
                        // query/fragment fields, alternate origins or URL credentials.
                        if !browser_verification || value != expected.as_str() {
                            return Err(Error::Contract);
                        }
                        Ok(Status::Pending(Some(value.clone())))
                    }
                    None if self.reason.is_none() => Ok(Status::Pending(None)),
                    None => Err(Error::Contract),
                }
            }
            EnrollmentState::Applied
                if self.verification_url.is_none() && self.reason.is_none() =>
            {
                Ok(Status::Applied)
            }
            EnrollmentState::Failed if self.verification_url.is_none() => Err(match self.reason {
                Some(EnrollmentReason::KeyRevoked) => Error::KeyRevoked,
                Some(EnrollmentReason::KeyExpired) => Error::KeyExpired,
                Some(EnrollmentReason::ApprovalExpired) => Error::ApprovalExpired,
                Some(EnrollmentReason::SyncFailed) | None => Error::SyncFailed,
                Some(EnrollmentReason::VerificationRequired) => Error::Contract,
            }),
            _ => Err(Error::Contract),
        }
    }
}
