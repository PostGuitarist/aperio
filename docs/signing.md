# Signing the installers

The Release workflow (`.github/workflows/release.yml`) signs the Mac app and the Windows installers once the secrets below are set on the repository. Without them it still builds: the Mac app is only ad-hoc signed, so macOS asks users to approve it in **System Settings → Privacy & Security** the first time they open it, and Windows shows a SmartScreen warning.

## macOS

Signing and notarizing a Mac app needs a membership of the [Apple Developer Program](https://developer.apple.com/programs/) ($99 a year). Once you have one:

1. **Make a Developer ID Application certificate.** In Xcode, open **Settings → Accounts**, pick your team, click **Manage Certificates**, then **+ → Developer ID Application**. (Or create it at [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates) from a certificate signing request made in Keychain Access.)
2. **Export it.** In Keychain Access, find "Developer ID Application: *Your Name* (*TEAMID*)" under **My Certificates**, right-click it, choose **Export**, save it as a `.p12`, and set a password.
3. **Make an app-specific password** for notarization at [account.apple.com](https://account.apple.com) → **Sign-In and Security → App-Specific Passwords**.
4. **Add the repository secrets** in **Settings → Secrets and variables → Actions → New repository secret**:

   | Secret | Value |
   | --- | --- |
   | `APPLE_CERTIFICATE` | the `.p12` file, base64-encoded: `base64 -i cert.p12 \| pbcopy` |
   | `APPLE_CERTIFICATE_PASSWORD` | the password you set when exporting |
   | `APPLE_SIGNING_IDENTITY` | the certificate's full name, e.g. `Developer ID Application: Jane Doe (AB12CD34EF)` |
   | `APPLE_ID` | your Apple Account email |
   | `APPLE_PASSWORD` | the app-specific password from step 3 |
   | `APPLE_TEAM_ID` | the 10-character team ID in the certificate's name |

The next release run imports the certificate into a temporary keychain, signs the app, and sends it to Apple for notarization, which usually takes a few minutes. The macOS build's log says which identity it signed with, and warns if the secrets are missing.

## Windows

The Windows installers are signed by [SignPath Foundation](https://signpath.org), which signs open-source projects for free. Until it's set up, the release workflow skips signing and publishes them unsigned.

Signing doesn't stop SmartScreen's "Windows protected your PC" warning straight away: SmartScreen trusts a publisher once enough people have installed its signed files, so the warning fades over the first few releases.

1. **Apply** at [signpath.org/apply](https://signpath.org/apply). SignPath checks the project against [its terms](https://signpath.org/terms); the README's [code signing policy](../README.md#code-signing-policy) is the page they ask for.
2. **Once approved**, in the SignPath project, add an artifact configuration that signs the installers inside the zip the workflow uploads, and make it the default:

   ```xml
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <directory path="nsis">
         <pe-file path="*.exe" max-matches="unbounded">
           <authenticode-sign/>
         </pe-file>
       </directory>
       <directory path="msi">
         <msi-file path="*.msi" max-matches="unbounded">
           <authenticode-sign/>
         </msi-file>
       </directory>
     </zip-file>
   </artifact-configuration>
   ```

   Link the project to this repository as a GitHub trusted build system.
3. **Add to the repository**, in **Settings → Secrets and variables → Actions**:

   | Name | Kind | Value |
   | --- | --- | --- |
   | `SIGNPATH_API_TOKEN` | secret | an API token for a SignPath user that can submit signing requests |
   | `SIGNPATH_ORGANIZATION_ID` | variable | your SignPath organization ID |
   | `SIGNPATH_PROJECT_SLUG` | variable | the project's slug, e.g. `aperio` |
   | `SIGNPATH_SIGNING_POLICY_SLUG` | variable | `release-signing` |

From then on, each release run sends the Windows installers to SignPath after the builds finish. SignPath asks you to approve the request on signpath.io, and the run waits up to an hour for that. If signing fails or isn't approved in time, the release isn't published, so an unsigned build never goes out by accident; run it again to retry.
