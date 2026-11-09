# COPR "make srpm" build method (only usable if the repository is public):
# COPR runs: make -f .copr/Makefile srpm outdir=<dir> spec=<path to .spec>
outdir ?= $(CURDIR)/dist
spec ?= packaging/fullscreen-ws.spec

srpm:
	dnf -y install cargo rust rpm-build git
	packaging/make-srpm.sh "$(basename $(notdir $(spec)))" "$(outdir)"
