# iOS encryption declaration

For the release owner submitting Nexus for iPhone to App Store Connect.
The first launch excludes all EU storefronts and China mainland, including France.

## What the shipping app uses

- HTTPS and Keychain through the operating system.
- Standard algorithms in JavaScript through `@noble/ciphers`, `@noble/curves` and `@noble/hashes`.
- AES-GCM, X25519, Ed25519, HMAC-SHA-256 and SHA-256 in `src/subtle.ts`.
- Synced Code sessions use these algorithms for device pairing and content decryption.
- `src/device-key.ts` stores the device secret in Keychain or Android Keystore.
- The app contains no proprietary encryption algorithm identified by this inventory.

## App Store Connect answers

1. Declare that the app uses encryption, including encryption from third-party libraries.
2. Select standard encryption algorithms implemented outside Apple's operating system.
3. Do not select proprietary or non-standard algorithms for the implementations listed above.
4. Confirm that France is unavailable, alongside the other EU storefronts and China mainland.
5. Complete Apple's determination for this build and record its result before App Review.

Apple's documentation table requires a French declaration for standard algorithms implemented outside the OS only when distributing in France.
It lists CCATS documentation for proprietary algorithms.
This determines Apple's upload documentation; it does not settle separate government reporting obligations.

The app omits `usesNonExemptEncryption`, so App Store Connect asks the questionnaire for each upload.
A value of `false` would claim exemption for the app and its third-party libraries.
Record Apple's determination before restoring that value.
If the determination requires documents, follow its instructions and use Apple's approved compliance code.
The prior README's OS-only explanation is obsolete.

## References

- [Apple's encryption documentation table](https://developer.apple.com/help/app-store-connect/reference/app-information/export-compliance-documentation-for-encryption/)
- [Apple's export compliance overview](https://developer.apple.com/help/app-store-connect/manage-app-information/overview-of-export-compliance/)
- [ITSAppUsesNonExemptEncryption](https://developer.apple.com/documentation/bundleresources/information-property-list/itsappusesnonexemptencryption)
- [Encryption export regulations and government reporting](https://developer.apple.com/documentation/security/complying-with-encryption-export-regulations)
