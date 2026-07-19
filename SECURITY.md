# Security Policy

## Reporting a vulnerability

Please email **yuvitbatra@gmail.com** with the details. Do not open a public
issue for security reports. You will get a response within 72 hours.

Include, if you can: affected component (CLI, registry, SDK, website), a
reproduction, and impact assessment.

## Scope

Reports of particular interest:

- Package archive handling: path traversal, decompression bombs, checksum
  bypass, cache poisoning in `~/.xelian`.
- Registry: authentication bypass, publishing to another user's namespace,
  writing outside the storage root.
- Runtime: escaping the declared permission set, running code before the
  first-run consent prompt.

## Supported versions

Pre-1.0: only the latest release is supported with security fixes.
