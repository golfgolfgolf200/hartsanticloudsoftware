# Terms of Service & Software End-User Agreement

**Effective Date:** November 17, 2026  
**Licensor:** Hart's Anti-Cloud Software (`hartsanticloudsoftware.com`)

---

## 1. Acceptance of Terms

By downloading, installing, or running Hart's Anti-Cloud Software (including
`hartsanticloud-supervisor`, `hartsanticloud-control-plane`, and bundled
scripts), you agree to these Terms and the accompanying
[LICENSE.md](LICENSE.md) (Apache License 2.0).

## 2. License Grant & Third-Party Components

- **First-Party Code:** Licensed to you under the **Apache License, Version
  2.0** (see [LICENSE.md](LICENSE.md)).
- **Third-Party Software & Model Weights:** Bundled or auto-provisioned
  third-party components (such as `llama.cpp`, Liquid AI `LFM2.5` model
  weights, Wasmtime, Firecracker, and SQLite) are governed by their respective
  open-source or model licenses listed in
  [THIRD_PARTY_PACKAGES.md](THIRD_PARTY_PACKAGES.md). You agree to comply with
  Liquid AI's LFM Open License v1.0 when running LFM2.5 weights.

## 3. Customer Responsibilities (Self-Hosted Infrastructure)

Because Hart's Anti-Cloud executes locally on hardware you own or control:

- **System & Physical Security:** You are responsible for maintaining host OS
  security updates, disk encryption, and physical access controls on machines
  running the software.
- **KVM & Sandbox Workloads:** You are responsible for the code and payloads
  you execute inside Firecracker microVMs or Wasmtime WASI sandboxes, and for
  ensuring you have authorization to process any data fed into local AI models.
- **AI Output Verification:** Local language models can produce inaccurate or
  incomplete outputs. You are responsible for reviewing AI-generated content
  before relying on it for financial, legal, medical, or operational decisions.

## 4. Disclaimer of Warranties

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE, AND NON-INFRINGEMENT.

## 5. Limitation of Liability

TO THE MAXIMUM EXTENT PERMITTED BY APPLICABLE LAW, IN NO EVENT SHALL HART'S
ANTI-CLOUD SOFTWARE OR ITS CONTRIBUTORS BE LIABLE FOR ANY INDIRECT, INCIDENTAL,
SPECIAL, CONSEQUENTIAL, OR PUNITIVE DAMAGES, OR ANY LOSS OF DATA, PROFITS, OR
BUSINESS INTERRUPTION ARISING OUT OF THE USE OR INABILITY TO USE THE SOFTWARE.

## 6. Governing Law

These Terms are governed by the laws of the State of Arkansas, United States,
without regard to conflict-of-law principles.
