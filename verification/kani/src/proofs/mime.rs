//! INV-MED-09 model-checking harnesses for the image MIME allowlist.

use eterea_logic::is_image_mime;

use crate::mime::{contract_violation, MUST_ACCEPT, MUST_REJECT};

/// INV-MED-09 regression floor: every allowlisted MIME (and its case/parameter
/// variants) is accepted and satisfies the contract helper.
#[kani::proof]
#[kani::unwind(32)]
fn image_mime_accepts_allowlist() {
    for mime in MUST_ACCEPT {
        assert!(
            is_image_mime(mime),
            "INV-MED-09: allowlisted MIME rejected"
        );
        assert!(
            contract_violation(mime).is_none(),
            "INV-MED-09: accepted MIME outside the allowlist"
        );
    }
}

/// INV-MED-09 regression floor: everything outside the decoder allowlist is
/// rejected, including lookalike types.
#[kani::proof]
#[kani::unwind(32)]
fn image_mime_rejects_other_types() {
    for mime in MUST_REJECT {
        assert!(
            !is_image_mime(mime),
            "INV-MED-09: non-image MIME accepted"
        );
    }
}
