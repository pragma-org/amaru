class Amaru < Formula
  desc "A Cardano blockchain node implementation"
  homepage "https://github.com/pragma-org/amaru"
  version "10.11.20260730"
  license "Apache-2.0"

  on_macos do
    depends_on arch: :arm64

    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260730/amaru-10.11.20260730-macos-aarch64.tar.gz"
      sha256 "b964c03edf5dee992f609d1a18980b8e5a982f1caaa80f926adb49e6691697fa"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260730/amaru-10.11.20260730-linux-aarch64.tar.gz"
      sha256 "53bfe0d2a33916665d0d1be4fbb288a7936c975caa223a0e6a3b0cc7fff58b43"
    end

    on_intel do
      url "https://github.com/pragma-org/amaru/releases/download/v10.11.20260730/amaru-10.11.20260730-linux-x86_64.tar.gz"
      sha256 "6f4c13dcf06490077c77888d5361bb5bec4adc685592379a02e5cec8ec768f56"
    end
  end

  def install
    root = if File.exist?("bin/amaru")
      Pathname.pwd
    else
      candidate = Dir["*/bin/amaru"].find { |entry| File.file?(entry) }
      candidate.nil? ? nil : Pathname.new(candidate).dirname.dirname
    end

    odie "expected extracted Amaru archive contents" if root.nil?

    bin.install root/"bin/amaru"
    man1.install root/"share/man/man1/amaru.1"
    bash_completion.install root/"share/bash-completion/completions/amaru"
    zsh_completion.install root/"share/zsh/site-functions/_amaru"
    fish_completion.install root/"share/fish/vendor_completions.d/amaru.fish"

    docs = root/"share/doc/amaru"
    if docs.directory?
      Dir[docs/"*"].sort.each do |path|
        pkgshare.install path
      end
    end
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/amaru --version")
  end
end
