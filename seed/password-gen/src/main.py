"""Generate strong passwords and passphrases: '16', 'passphrase 4', 'pin 6'."""
import secrets
import string
import sys

WORDS = ("anchor apple arrow autumn basil beacon breeze candle canyon cedar "
         "cloud comet copper coral crane crystal delta drift ember falcon "
         "fern flint forest garnet glacier grove harbor hazel indigo iris "
         "jasper juniper kestrel lagoon lantern lark linen lotus maple "
         "meadow mesa nectar nova oasis obsidian onyx opal orchard osprey "
         "pearl pebble pine plume prairie quartz quill raven reef ridge "
         "river rowan saffron sage sequoia shadow sierra slate sparrow "
         "spruce summit thistle timber topaz tulip tundra velvet violet "
         "walnut willow wren zephyr zinc").split()
ALPHABET = string.ascii_letters + string.digits + "!@#$%^&*-_=+"

for line in sys.stdin:
    parts = line.strip().lower().split()
    try:
        if not parts or parts[0].isdigit():
            n = int(parts[0]) if parts else 20
            n = max(8, min(n, 128))
            print("".join(secrets.choice(ALPHABET) for _ in range(n)), flush=True)
        elif parts[0] == "passphrase":
            n = max(3, min(int(parts[1]) if len(parts) > 1 else 4, 12))
            print("-".join(secrets.choice(WORDS) for _ in range(n)), flush=True)
        elif parts[0] == "pin":
            n = max(4, min(int(parts[1]) if len(parts) > 1 else 6, 12))
            print("".join(secrets.choice(string.digits) for _ in range(n)), flush=True)
        else:
            print("usage: '<length>' | 'passphrase <words>' | 'pin <digits>'", flush=True)
    except ValueError:
        print("usage: '<length>' | 'passphrase <words>' | 'pin <digits>'", flush=True)
