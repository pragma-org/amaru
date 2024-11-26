---
type: process
status: proposed
---

# Git Etiquette

## Motivation

In a large project with multiple contributors, it is necessary to establish some git etiquette to keep the project maintainable. The following are the guiding principles:

- make git history more readable
- allow automatic changelog generation
- promote self-contained PRs that are easier to review
- promote continuous integration

## Decision

Contributors will follow these git practices to keep the project maintainable.

- use trunk-based development
- maintain a linear history on trunk (`main` branch)
- apply changes via PRs
- use squash merge for all PRs
- use conventional commits for git messages
- all commits must be either gpg-signed or at the very least "signed-off" (`-s` option on git), acting as a [Developer Certificate of Origin (DCO)](developercertificate.org):
  ```
  Developer Certificate of Origin
  Version 1.1
  
  Copyright (C) 2004, 2006 The Linux Foundation and its contributors.
  
  Everyone is permitted to copy and distribute verbatim copies of this
  license document, but changing it is not allowed.
  
  
  Developer's Certificate of Origin 1.1
  
  By making a contribution to this project, I certify that:
  
  (a) The contribution was created in whole or in part by me and I
      have the right to submit it under the open source license
      indicated in the file; or
  
  (b) The contribution is based upon previous work that, to the best
      of my knowledge, is covered under an appropriate open source
      license and I have the right under that license to submit that
      work with modifications, whether created in whole or in part
      by me, under the same open source license (unless I am
      permitted to submit under a different license), as indicated
      in the file; or
  
  (c) The contribution was provided directly to me by some other
      person who certified (a), (b) or (c) and I have not modified
      it.
  
  (d) I understand and agree that this project and the contribution
      are public and that a record of the contribution (including all
      personal information I submit with it, including my sign-off) is
      maintained indefinitely and may be redistributed consistent with
      this project or the open source license(s) involved.
  ```  
