class Repod < Formula
  desc "Process repositories, generate trees, and export analyzed contents"
  homepage "https://github.com/iskng/repod"
  url "https://github.com/iskng/repod/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_SHA256"
  license "MIT"

  depends_on "rust" => :build
  depends_on "pkg-config" => :build
  depends_on "openssl@3"

  def install
    # Build from source and install the single binary
    system "cargo", "build", "--release", "--locked"
    bin.install "target/release/repod"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/repod -V")
  end
end
