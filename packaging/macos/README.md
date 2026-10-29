# macOS distribution: signing, notarization, Homebrew

How DBCrust reaches a Mac without compiling anything and without Gatekeeper
workarounds, and the one-time setup it needs. Maintainers only: users read
[Installation](../../docs/src/content/docs/installation.mdx).

| Artefact | Built by | Signed with | Installs via |
|---|---|---|---|
| `dbcrust-{aarch64,x86_64}-apple-darwin.tar.gz` (`dbcrust`, `dbc`, `dbcrust-agent`) | `native-binaries-macos` | Developer ID, hardened runtime, notarized (online ticket: a bare executable cannot be stapled) | `install.sh`, Homebrew formula, manual download |
| `DBCrust-macos-universal.dmg` (`DBCrust.app`, Apple silicon + Intel) | `gui-macos` | Developer ID, hardened runtime, notarized, ticket stapled to the app and the DMG | release page, Homebrew cask |

Both are packaged by [`package.sh`](package.sh) in the `package-macos-signed`
job of [`build-binaries.yml`](../../.github/workflows/build-binaries.yml), the
only job that sees the signing secrets. Until the repository variable
`MACOS_SIGNING` is `on`, `package-macos` runs instead: ad-hoc signatures, the CLI
archives are published as before and the DMG is **not** published (Gatekeeper
blocks an ad-hoc signed app that was downloaded).

## 1. Apple Developer Program (personal account)

Use your **own** Apple Account, not an employer's. Everything below happens in
your own team: if your Apple Account is also a member of another team, pick
yours in the team switcher (top right of developer.apple.com) and check the
Team ID on the Membership page each time.

1. Turn on two-factor authentication for the Apple Account.
2. Enroll as an **Individual** at <https://developer.apple.com/programs/enroll/>,
   or faster in the Apple Developer app (iPhone/Mac: Account → Enroll), which
   verifies your ID document. 99 USD a year (local price in the EU). Your legal
   name becomes the publisher name in the certificate:
   `Developer ID Application: Firstname Lastname (TEAMID)`.
3. Wait for the confirmation e-mail (often within 48 hours).

Nothing else from the program is needed: no App ID, no provisioning profile, no
App Sandbox. Developer ID distribution outside the App Store only needs the
certificate and notarization.

## 2. Developer ID Application certificate

The private key is created on your Mac and never leaves it except as the
password-protected `.p12` you give CI. Create it on a **personal** Mac: an
employer-managed one (device management, backups, endpoint agents, handed back
when you leave) is the wrong home for it, and its keychain may already hold the
employer's identity. Steps 1 and 5–6 need no secret on the machine and can be
done from anywhere; the rehearsal in section 4 can run in CI instead.

1. Keychain Access → Keychain Access menu → Certificate Assistant → **Request a
   Certificate From a Certificate Authority…**: your e-mail, a common name such
   as `DBCrust Developer ID`, **Saved to disk**. The key pair lands in your login
   keychain.
2. <https://developer.apple.com/account/resources/certificates/add> (your team)
   → **Developer ID Application** → profile type **G2 Sub-CA** (the original
   Developer ID intermediate expires on 1 February 2027) → upload the
   `.certSigningRequest` → download `developerID_application.cer` and
   double-click it.
3. Check it, and note the identity's SHA-1 hash:

   ```bash
   security find-identity -v -p codesigning | grep "Developer ID Application"
   ```

   Several identities may show up (an employer's included). From here on, always
   name **yours** explicitly: by its hash or its full name with *your* Team ID.
   `package.sh` never picks one by itself.
4. Export for CI: Keychain Access → login → My Certificates → right-click
   *Developer ID Application: … (YOURTEAMID)* → **Export** → `.p12`, with a long
   random password. Keep the `.p12` and its password in your password manager,
   and note the certificate's expiry date: certificates issued from G2 are valid
   **one year** (section 7).
5. To sign on another Mac, bring the same `.p12` there (a 1Password document
   item, its password in the same item, works well), double-click it and type the
   password: it lands in that Mac's login keychain, and step 3 shows it. If it is
   listed as not valid (`CSSMERR_TP_NOT_TRUSTED`), install Apple's
   [Developer ID G2 intermediate](https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer)
   (double-click it too). `codesign` only signs from a macOS keychain, so the
   password manager stores and carries the identity; it cannot sign by itself.

## 3. Notarization key (App Store Connect API)

1. <https://appstoreconnect.apple.com/access/integrations/api> (Users and Access
   → Integrations → App Store Connect API). The first time, the account holder
   (you) clicks **Request Access** and accepts the terms.
2. **Team Keys** tab → **+** → name `dbcrust-notary`, access **Developer** →
   Generate. Use a *team* key: individual keys only notarize with Xcode 26+ and
   without an issuer ID.
3. **Download** `AuthKey_<KEYID>.p8`. Apple lets you download it only once. Note
   the **Key ID** and the **Issuer ID** (the UUID above the list).
4. For local notarization, store it in your keychain under a profile name:

   ```bash
   xcrun notarytool store-credentials dbcrust-notary \
     --key AuthKey_<KEYID>.p8 --key-id <KEYID> --issuer <ISSUER-UUID>
   ```

`notarytool` and `stapler` come with the Command Line Tools: Xcode is not needed.

## 4. Rehearse locally

Build this Mac's architecture, sign, notarize and package exactly like CI:

```bash
export MACOS_SIGNING_IDENTITY=<SHA-1 of your identity>   # step 2.3
export NOTARY_PROFILE=dbcrust-notary                     # step 3.4
mise run macos:package                                   # → target/macos-dist/
```

The script fails loudly on anything a user would hit: a library outside the OS
(`otool -L`), a rejected notarization (it prints Apple's log), or a Gatekeeper
refusal (`spctl`). To try the app as a downloader would, mark the DMG
quarantined and open it (`curl` downloads are not quarantined, browsers' are):

```bash
xattr -w com.apple.quarantine "0081;$(printf %x "$(date +%s)");Safari;" target/macos-dist/DBCrust-macos-aarch64.dmg
open target/macos-dist/DBCrust-macos-aarch64.dmg
```

Without `MACOS_SIGNING_IDENTITY` the same task produces ad-hoc signed packages,
which is enough to test the app bundle itself.

**Or rehearse in CI**, so the certificate never has to be on a laptop: once
section 5 is set up, push a pre-release tag from `main`. The Release workflow
signs and notarizes exactly as for a release and publishes a GitHub
*pre-release* (never `latest`, so the download links stay on the last stable
release); PyPI and the Homebrew tap skip tags with a `-`. Download the DMG with
a browser to see what Gatekeeper says, then remove it:

```bash
git tag v0.40.0-rc.1 && git push origin v0.40.0-rc.1
gh release delete v0.40.0-rc.1 --cleanup-tag --yes -R clement-tourriere/dbcrust   # afterwards
```

## 5. GitHub configuration

**Environment** (Settings → Environments → New environment) named
`macos-signing`:

- Deployment branches and tags → *Selected branches and tags* → add the **tag**
  rule `v*`. Only release tags can then use the secrets; a pull request (from a
  fork or not) never reaches the signing job.
- Optional: *Required reviewers* = you, to approve each release's signing run.
- Environment secrets:

  ```bash
  repo=clement-tourriere/dbcrust
  base64 -i DeveloperID.p12 | gh secret set MACOS_CERTIFICATE_P12 -R $repo --env macos-signing
  gh secret set MACOS_CERTIFICATE_PASSWORD -R $repo --env macos-signing   # prompts
  gh secret set APPLE_API_KEY_P8 -R $repo --env macos-signing < AuthKey_<KEYID>.p8
  gh secret set APPLE_API_KEY_ID -R $repo --env macos-signing --body <KEYID>
  gh secret set APPLE_API_ISSUER_ID -R $repo --env macos-signing --body <ISSUER-UUID>
  ```

Then switch signing on. From the next release, a signing or notarization
failure fails the release instead of shipping unsigned packages:

```bash
gh variable set MACOS_SIGNING -R clement-tourriere/dbcrust --body on
```

A manual `workflow_dispatch` of *Release* runs from `main`, which the `v*` rule
refuses: re-run the tag's own run instead (Actions → the run → Re-run jobs).

## 6. Homebrew tap

1. Create the public repository `clement-tourriere/homebrew-tap` (a README is
   enough; the workflow creates `Formula/` and `Casks/`).
2. Create a fine-grained token: Settings → Developer settings → Fine-grained
   tokens → repository access *Only select repositories* →
   `homebrew-tap`; permission **Contents: Read and write**; one-year expiry
   (set a reminder).
3. Configure the release workflow:

   ```bash
   gh secret set HOMEBREW_TAP_TOKEN -R clement-tourriere/dbcrust               # paste the token
   gh variable set HOMEBREW_TAP_REPO -R clement-tourriere/dbcrust --body clement-tourriere/homebrew-tap
   ```

Each stable release then commits `Formula/dbcrust.rb` (CLI: macOS and Linux)
and, once the DMG is published, `Casks/dbcrust-gui.rb`, both rendered by
[`render.py`](../homebrew/render.py) from the published checksums. Users run:

```bash
brew install clement-tourriere/tap/dbcrust              # CLI: dbcrust, dbc, dbcrust-agent
brew install --cask clement-tourriere/tap/dbcrust-gui   # desktop app
```

To publish the formula for an existing release without waiting for the next one:

```bash
gh release download v0.40.0 -R clement-tourriere/dbcrust --pattern '*.sha256' -D /tmp/sums
python3 packaging/homebrew/render.py --version 0.40.0 --assets /tmp/sums --tap ../homebrew-tap
```

## 7. Keeping it running

- **Membership** renews yearly. If it lapses, published releases keep working;
  new ones cannot be notarized.
- **Certificate**: a G2 Developer ID certificate is valid one year. A few weeks
  before it expires, create the next one (step 2; you may hold up to five at a
  time) and replace the two certificate secrets. Releases signed while it was
  valid keep installing and running after it expires. The Team ID does not
  change, so keychain approvals and the local network permission users granted
  carry over.
- **API key**: revoke and recreate any time (step 3), then update the three
  `APPLE_API_*` secrets.
- **A leaked `.p12`**: ask Apple to revoke the certificate through Developer
  Support → *Certificates* (it cannot be revoked from the portal). Revocation
  stops **every** app signed with it from installing *and* from launching, even
  installed copies: create a new certificate, re-sign, re-notarize and
  re-publish the current release, and tell users to update.

## Troubleshooting

| Symptom | Cause |
|---|---|
| `MACOS_CERTIFICATE_P12 holds no valid Developer ID Application identity` | Exported the certificate without its private key, or a *Developer ID Installer* / *Apple Development* certificate. |
| `errSecInternalComponent` while signing in CI | The key's partition list or a locked keychain: the import step must have run in the same job. |
| notarization `Invalid` | The script prints Apple's log. Usually an unsigned nested binary, a missing hardened runtime (`--options runtime`) or timestamp. |
| notarytool HTTP 401 | Wrong Key ID or Issuer ID, an *individual* key used with an issuer ID, or a revoked key. |
| "dbcrust wants to use your confidential information stored in dbcrust.store" once after upgrading | Expected the first time a Developer ID build reads a password saved by an unsigned build: **Always Allow** sticks for later updates. |
| The app cannot reach a database on the LAN or a container's address | macOS 15+ local network privacy: the user allows DBCrust in System Settings → Privacy & Security → Local Network. Terminal CLIs are exempt. |
