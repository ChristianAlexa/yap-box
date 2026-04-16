NPM := bash -c 'source ~/.nvm/nvm.sh && npm "$$@"' --

.PHONY: dev web build build-web install

dev:
	$(NPM) run dev:tauri

web:
	$(NPM) run dev

build:
	$(NPM) run build:tauri

build-web:
	$(NPM) run build

install:
	$(NPM) install
