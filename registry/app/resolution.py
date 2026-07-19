import semver

from .models import VersionRecord


def resolve_latest(versions: list[VersionRecord]) -> VersionRecord | None:
    """Resolve to the highest SemVer that is not yanked and not pre-release.

    Args:
        versions: All version records for a package, including yanked ones.

    Returns:
        The best matching version, or None if no resolvable version exists.
    """
    best: tuple[semver.Version, VersionRecord] | None = None
    for v in versions:
        if v.yanked:
            continue
        try:
            parsed = semver.Version.parse(v.version)
        except ValueError:
            continue
        if parsed.prerelease:
            continue
        if best is None or parsed > best[0]:
            best = (parsed, v)
    return best[1] if best else None
