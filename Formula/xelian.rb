# Homebrew formula for the Xelian CLI.
#
# Usage (after this is in a tap, e.g. yuvitbatra/homebrew-xelian):
#   brew tap yuvitbatra/xelian
#   brew install xelian
#
# Regenerate on each release: bump `version`, update the four `sha256`s from
# the release's *.tar.gz.sha256 assets (or run scripts/update_formula.sh).
class Xelian < Formula
  desc "Local-first registry and runtime for AI agents and MCP servers"
  homepage "https://github.com/yuvitbatra/Xelian"
  version "0.1.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/yuvitbatra/Xelian/releases/download/v0.1.1/xelian-aarch64-apple-darwin.tar.gz"
      sha256 "255f5168782289f68c40cd07d2567ca5073d13b2e9e9b26d48c12862febe57f2"
    end
    on_intel do
      url "https://github.com/yuvitbatra/Xelian/releases/download/v0.1.1/xelian-x86_64-apple-darwin.tar.gz"
      sha256 "a7b3d8dad7444d4b3e00c573e3ae5c02e7d2799849d1f0849b8db4e33d9558cc"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/yuvitbatra/Xelian/releases/download/v0.1.1/xelian-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "949404f6f733d44392ecfa555d4bf7d75edf8efa3ad7d2404b1b72da80b8ee5d"
    end
    on_intel do
      url "https://github.com/yuvitbatra/Xelian/releases/download/v0.1.1/xelian-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "14b25a6911710e2fd57d20213ee30c1502b19e078a1d3f8b0b69138116e045cd"
    end
  end

  def install
    bin.install "xelian"
  end

  test do
    assert_match "xelian", shell_output("#{bin}/xelian --version")
  end
end
