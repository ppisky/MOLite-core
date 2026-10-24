# Security

Security fixes target the current development version and latest release. Do not include passwords, provider keys, private MOC files or personal databases in public issues.

Please use GitHub's private vulnerability reporting under this repository's Security tab. Include the affected revision, platform, reproduction and expected impact. For ordinary bugs use the issue tracker.

Credentials are read from environment variables. Non-loopback HTTP listeners require a token; deployments still need an appropriate transport and access boundary. Native C callers must obey the pointer and buffer ownership contract.
