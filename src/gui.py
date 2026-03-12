"""
Rivals Swapper GUI — built with customtkinter.

Modern two-screen layout with hero portrait images and skin icons:
  Screen 1 — Hero Grid : scrollable grid of hero portrait cards.
  Screen 2 — Skin Grid : scrollable grid of skin cards with swap buttons.

State:
  • One swap per character (must Remove before swapping another).
  • Active swaps persisted in settings.json.
  • Removing a swap deletes deployed .pak/.utoc/.ucas.
  • Logs clear on every new swap.
"""

from __future__ import annotations

import threading
from pathlib import Path
from typing import Optional

import customtkinter as ctk
from tkinter import filedialog
from PIL import Image

from .image_cache import ImageCache
from .skin_database import SkinDatabase, CharacterInfo, SkinInfo
from .swap_engine import SwapEngine
from .retoc_wrapper import RetocWrapper
from .uassettool_wrapper import UAssetToolWrapper
from .settings import (
    Settings, SwapRecord, load_settings, save_settings,
)


# ======================================================================
# Theme & constants
# ======================================================================

ctk.set_appearance_mode("dark")
ctk.set_default_color_theme("blue")

# Grid
HERO_COLUMNS   = 5
HERO_CARD_W    = 200
HERO_CARD_H    = 200
HERO_IMG_SIZE  = (150, 150)   # portrait thumbnail
SKIN_COLUMNS   = 5
SKIN_IMG_SIZE  = (120, 120)   # skin card thumbnail

# Colours — dark-gray + yellow accent
_BG_DARK    = "#121212"
_BG_CARD    = "#1e1e1e"
_BG_CARD_A  = "#1e2a1e"       # active swap card
_BG_HOVER   = "#2a2a2a"
_ACCENT     = "#f0c232"       # yellow accent
_ACCENT_H   = "#f5d45a"
_GREEN      = "#00b894"
_GREEN_H    = "#00d9a7"
_RED        = "#d63031"
_RED_H      = "#e84343"
_BLUE       = "#0984e3"
_BLUE_H     = "#2d9bf0"
_GREY       = "#555555"
_TEXT        = "#e0e0e0"
_TEXT_DIM    = "#808080"
_BORDER     = "#333333"
_ACTIVE_BDG = "#00b894"

# Placeholder image (generated once)
_PLACEHOLDER: Optional[Image.Image] = None


def _get_placeholder(size: tuple[int, int] = (120, 120)) -> Image.Image:
    """Create a simple dark placeholder image."""
    global _PLACEHOLDER
    if _PLACEHOLDER is None or _PLACEHOLDER.size != size:
        _PLACEHOLDER = Image.new("RGBA", size, (30, 30, 30, 255))
    return _PLACEHOLDER.resize(size, Image.LANCZOS)


# ======================================================================
# Settings dialog
# ======================================================================

class SettingsWindow(ctk.CTkToplevel):
    """Modal settings dialog."""

    def __init__(self, master, settings: Settings, on_save=None):
        super().__init__(master)
        self.title("Settings")
        self.geometry("600x350")
        self.resizable(False, False)
        self.configure(fg_color=_BG_DARK)
        self.settings = settings
        self.on_save = on_save
        self._build_ui()
        self.grab_set()

    def _build_ui(self) -> None:
        pad = {"padx": 18, "pady": 8}

        ctk.CTkLabel(self, text="⚙  Settings",
                      font=ctk.CTkFont(size=20, weight="bold"),
                      text_color=_TEXT).pack(pady=(18, 10))

        frame = ctk.CTkFrame(self, fg_color=_BG_CARD, corner_radius=12)
        frame.pack(fill="x", padx=24, pady=4)

        ctk.CTkLabel(frame, text="Game Paks Folder",
                      text_color=_TEXT_DIM).grid(
            row=0, column=0, sticky="w", **pad)
        self.paks_var = ctk.StringVar(value=self.settings.game_paks_dir)
        ctk.CTkEntry(frame, textvariable=self.paks_var, width=320,
                      fg_color=_BG_DARK, border_color=_BORDER,
                      text_color=_TEXT).grid(
            row=0, column=1, **pad)
        ctk.CTkButton(frame, text="Browse", width=80,
                       fg_color=_ACCENT, hover_color=_ACCENT_H,
                       command=lambda: self._browse(self.paks_var)).grid(
            row=0, column=2, **pad)

        self.deploy_var = ctk.BooleanVar(value=self.settings.auto_deploy)
        ctk.CTkCheckBox(frame, text="Auto-deploy to ~mods after packing",
                         variable=self.deploy_var,
                         text_color=_TEXT,
                         fg_color=_ACCENT, hover_color=_ACCENT_H).grid(
            row=1, column=0, columnspan=3, sticky="w", **pad)

        ctk.CTkLabel(frame, text="API Key (marvelrivalsapi.com)",
                      text_color=_TEXT_DIM).grid(
            row=2, column=0, sticky="w", **pad)
        self.api_key_var = ctk.StringVar(value=self.settings.api_key)
        ctk.CTkEntry(frame, textvariable=self.api_key_var, width=320,
                      fg_color=_BG_DARK, border_color=_BORDER,
                      text_color=_TEXT, show="•").grid(
            row=2, column=1, columnspan=2, **pad)

        bf = ctk.CTkFrame(self, fg_color="transparent")
        bf.pack(pady=18)
        ctk.CTkButton(bf, text="Save", width=130, height=36,
                       fg_color=_ACCENT, hover_color=_ACCENT_H,
                       text_color="#1a1a1a",
                       command=self._save).pack(side="left", padx=8)
        ctk.CTkButton(bf, text="Cancel", width=130, height=36,
                       fg_color=_GREY, hover_color=_BG_HOVER,
                       text_color=_TEXT,
                       command=self.destroy).pack(side="left", padx=8)

    def _browse(self, var):
        p = filedialog.askdirectory()
        if p:
            var.set(p)

    def _save(self):
        self.settings.game_paks_dir = self.paks_var.get().strip()
        self.settings.auto_deploy = self.deploy_var.get()
        self.settings.api_key = self.api_key_var.get().strip()
        save_settings(self.settings)
        if self.on_save:
            self.on_save()
        self.destroy()


# ======================================================================
# Main window
# ======================================================================

class App(ctk.CTk):
    WIDTH  = 1280
    HEIGHT = 850

    def __init__(self) -> None:
        super().__init__()
        self.title("Rivals Swapper")
        self.geometry(f"{self.WIDTH}x{self.HEIGHT}")
        self.minsize(1000, 700)
        self.configure(fg_color=_BG_DARK)

        # Centre window on screen
        self.update_idletasks()
        sw = self.winfo_screenwidth()
        sh = self.winfo_screenheight()
        x = (sw - self.WIDTH) // 2
        y = (sh - self.HEIGHT) // 2
        self.geometry(f"{self.WIDTH}x{self.HEIGHT}+{x}+{y}")

        # Window icon
        _icon_path = Path(__file__).resolve().parent.parent / "assets" / "RivalsIcon_NoOutline.ico"
        if _icon_path.exists():
            self.iconbitmap(str(_icon_path))
            self.after(200, lambda: self.iconbitmap(str(_icon_path)))

        # Load logo for loading screen
        _logo_path = Path(__file__).resolve().parent.parent / "assets" / "RivalsIcon_NoOutline.png"
        self._logo_pil: Image.Image | None = None
        if _logo_path.exists():
            try:
                self._logo_pil = Image.open(_logo_path).convert("RGBA")
            except Exception:
                pass

        self.settings = load_settings()
        self.db = SkinDatabase()
        self.img_cache = ImageCache()
        self.output_dir = Path(__file__).resolve().parent.parent / "output"
        self._current_char: CharacterInfo | None = None
        self._busy = False

        # CTkImage references (prevent GC)
        self._ctk_images: dict[str, ctk.CTkImage] = {}

        # Show loading screen, then start background preload
        self._build_loading_screen()
        threading.Thread(target=self._preload_all, daemon=True).start()

    # ------------------------------------------------------------------
    # Loading screen
    # ------------------------------------------------------------------

    def _build_loading_screen(self) -> None:
        """Show a centred loading screen with logo and progress bar."""
        self._loading_frame = ctk.CTkFrame(self, fg_color=_BG_DARK)
        self._loading_frame.place(relx=0, rely=0, relwidth=1, relheight=1)

        inner = ctk.CTkFrame(self._loading_frame, fg_color="transparent")
        inner.place(relx=0.5, rely=0.45, anchor="center")

        # Logo
        if self._logo_pil is not None:
            logo_size = (128, 128)
            self._logo_ctk = ctk.CTkImage(
                light_image=self._logo_pil,
                dark_image=self._logo_pil,
                size=logo_size,
            )
            ctk.CTkLabel(inner, image=self._logo_ctk, text="").pack(pady=(0, 16))

        ctk.CTkLabel(
            inner, text="Rivals Swapper",
            font=ctk.CTkFont(size=40, weight="bold"),
            text_color=_ACCENT,
        ).pack(pady=(0, 8))

        self._load_status = ctk.CTkLabel(
            inner, text="Loading skin database…",
            font=ctk.CTkFont(size=16),
            text_color=_TEXT_DIM,
        )
        self._load_status.pack(pady=(0, 20))

        self._load_bar = ctk.CTkProgressBar(
            inner, width=450, height=12,
            fg_color=_BG_CARD, progress_color=_ACCENT,
            corner_radius=6,
        )
        self._load_bar.set(0)
        self._load_bar.pack()

        self._load_detail = ctk.CTkLabel(
            inner, text="",
            font=ctk.CTkFont(size=13),
            text_color=_TEXT_DIM,
        )
        self._load_detail.pack(pady=(12, 0))

    def _set_load_status(self, text: str, detail: str = "",
                         progress: float | None = None) -> None:
        """Thread-safe update for the loading screen labels + bar."""
        def _update():
            try:
                self._load_status.configure(text=text)
                self._load_detail.configure(text=detail)
                if progress is not None:
                    self._load_bar.set(progress)
            except Exception:
                pass
        self.after(0, _update)

    def _preload_all(self) -> None:
        """Background thread: fetch skin DB → costumes API → download all images."""
        api_key = self.settings.api_key

        # --- Step 1: Skin database (one API call) ---
        self._set_load_status("Loading skin database…", progress=0.0)
        ok = self.db.fetch_from_api()
        if not ok:
            self._set_load_status("⚠  Could not reach skin database",
                                  "Starting with offline data…")
            self.after(1500, self._finish_loading)
            return

        char_names = self.db.get_character_names()

        # --- Build hero + skin data ---
        heroes = []
        skin_ids_by_hero: dict[str, list[str]] = {}
        for name in char_names:
            char = self.db.get_character(name)
            if char:
                heroes.append((name, char.default_skin_id))
                skins = self.db.get_skins(name)
                if skins:
                    skin_ids_by_hero[name] = [s.skin_id for s in skins]

        # --- Step 2: Build slug map (one fast API call) ---
        self._set_load_status("Resolving hero names…", progress=0.03)
        self.img_cache.build_slug_map(api_key, char_names)

        # --- Quick-check: skip heavy work if all API-backed portraits are cached ---
        api_heroes = self.img_cache.get_api_hero_names()
        all_portraits_cached = all(
            self.img_cache.has(self.img_cache.hero_portrait_key(name))
            for name, _ in heroes
            if name in api_heroes
        )

        if all_portraits_cached:
            # Portraits all present — skin icons are best-effort, go to UI
            self._set_load_status("Ready!", progress=1.0)
            self.after(100, self._finish_loading)
            return

        # --- Step 3: Fetch ALL costumes JSON in parallel (~47 API calls) ---
        self._set_load_status("Fetching costume data…",
                              f"0 / {len(char_names)}", progress=0.05)

        def _costumes_progress(done: int, total: int) -> None:
            frac = 0.05 + 0.35 * (done / max(total, 1))
            self._set_load_status(
                "Fetching costume data…",
                f"{done} / {total} heroes",
                progress=frac,
            )

        self.img_cache.fetch_all_costumes(
            api_key, char_names,
            progress_callback=_costumes_progress,
        )

        # --- Step 4: Collect + download missing images ---
        self._set_load_status("Preparing downloads…", progress=0.40)
        tasks = self.img_cache.collect_download_tasks(heroes, skin_ids_by_hero)

        total_imgs = len(tasks)
        if total_imgs:
            self._set_load_status("Downloading images…",
                                  f"0 / {total_imgs}", progress=0.42)

            def _img_progress(done: int, total: int) -> None:
                frac = 0.42 + 0.55 * (done / max(total, 1))
                self._set_load_status(
                    "Downloading images…",
                    f"{done} / {total}",
                    progress=frac,
                )

            self.img_cache.batch_download_images(
                tasks, progress_callback=_img_progress,
            )

        # --- Done ---
        self._set_load_status("Ready!", progress=1.0)
        self.after(250, self._finish_loading)

    def _finish_loading(self) -> None:
        """Tear down loading screen, build the real UI."""
        self._loading_frame.destroy()
        self._build_shell()
        self._show_hero_grid()

    # ------------------------------------------------------------------
    # CTkImage helper
    # ------------------------------------------------------------------

    def _get_ctk_image(
        self, key: str, size: tuple[int, int]
    ) -> ctk.CTkImage:
        """Return a CTkImage for the given cache key, or a placeholder."""
        cache_key = f"{key}_{size[0]}x{size[1]}"
        if cache_key in self._ctk_images:
            return self._ctk_images[cache_key]

        pil = self.img_cache.get(key)
        if pil is None:
            pil = _get_placeholder(size)

        ctk_img = ctk.CTkImage(light_image=pil, dark_image=pil, size=size)
        self._ctk_images[cache_key] = ctk_img
        return ctk_img

    def _invalidate_ctk_images(self) -> None:
        """Clear cached CTkImage references so they get rebuilt."""
        self._ctk_images.clear()

    def _update_ctk_images_in_place(self) -> None:
        """Update existing CTkImage objects with newly downloaded PIL data.
        No widget rebuild needed — Tk redraws automatically."""
        for cache_key, ctk_img in list(self._ctk_images.items()):
            # cache_key = "{img_key}_{W}x{H}"
            parts = cache_key.rsplit("_", 1)
            if len(parts) != 2:
                continue
            img_key = parts[0]
            pil = self.img_cache.get(img_key)
            if pil is not None:
                try:
                    ctk_img.configure(light_image=pil, dark_image=pil)
                except Exception:
                    pass

    # ------------------------------------------------------------------
    # Outer shell (always visible)
    # ------------------------------------------------------------------

    def _build_shell(self) -> None:
        # Top bar
        top = ctk.CTkFrame(self, fg_color=_BG_CARD, corner_radius=0, height=56)
        top.pack(fill="x")
        top.pack_propagate(False)

        ctk.CTkLabel(
            top, text="  Rivals Swapper",
            font=ctk.CTkFont(size=22, weight="bold"),
            text_color=_ACCENT,
        ).pack(side="left", padx=16)

        ctk.CTkButton(
            top, text="⚙  Settings", width=120, height=34,
            fg_color="transparent", hover_color=_BG_HOVER,
            border_width=1, border_color=_BORDER,
            text_color=_TEXT,
            command=self._open_settings,
        ).pack(side="right", padx=16)

        self._log_toggle_btn = ctk.CTkButton(
            top, text="📋  Log", width=90, height=34,
            fg_color="transparent", hover_color=_BG_HOVER,
            border_width=1, border_color=_BORDER,
            text_color=_TEXT,
            command=self._toggle_log,
        )
        self._log_toggle_btn.pack(side="right", padx=4)

        # Content area
        self.content = ctk.CTkFrame(self, fg_color="transparent")
        self.content.pack(fill="both", expand=True, padx=20, pady=(12, 0))

        # Log panel (hidden by default)
        self._log_visible = False
        self.log_frame = ctk.CTkFrame(self, fg_color=_BG_CARD, corner_radius=10)
        # Don't pack yet — hidden by default

        log_hdr = ctk.CTkFrame(self.log_frame, fg_color="transparent")
        log_hdr.pack(fill="x", padx=10, pady=(6, 0))

        ctk.CTkLabel(
            log_hdr, text="  Log",
            font=ctk.CTkFont(size=12, weight="bold"),
            text_color=_TEXT_DIM,
        ).pack(side="left")

        ctk.CTkButton(
            log_hdr, text="✕", width=28, height=28,
            fg_color="transparent", hover_color=_BG_HOVER,
            text_color=_TEXT_DIM,
            command=self._toggle_log,
        ).pack(side="right")

        self.log_box = ctk.CTkTextbox(
            self.log_frame, height=140, state="disabled",
            fg_color=_BG_DARK, border_width=0,
            font=ctk.CTkFont(family="Consolas", size=12),
            text_color=_TEXT_DIM,
        )
        self.log_box.pack(fill="x", padx=10, pady=(2, 10))

    def _toggle_log(self) -> None:
        """Show or hide the log panel."""
        if self._log_visible:
            self.log_frame.pack_forget()
            self._log_visible = False
        else:
            self.log_frame.pack(fill="x", padx=20, pady=(8, 14))
            self._log_visible = True

    def _show_log(self) -> None:
        """Ensure the log panel is visible."""
        if not self._log_visible:
            self._toggle_log()

    # ------------------------------------------------------------------
    # Screen 1 — Hero grid
    # ------------------------------------------------------------------

    def _show_hero_grid(self) -> None:
        self._current_char = None
        self._clear_content()

        # Header
        hdr = ctk.CTkFrame(self.content, fg_color="transparent")
        hdr.pack(fill="x", pady=(0, 10))
        ctk.CTkLabel(
            hdr, text="Select a Hero",
            font=ctk.CTkFont(size=18, weight="bold"),
            text_color=_TEXT,
        ).pack(side="left")

        active_count = len(self.settings.active_swaps)
        if active_count:
            ctk.CTkLabel(
                hdr,
                text=f"  {active_count} active swap{'s' if active_count > 1 else ''}",
                font=ctk.CTkFont(size=13),
                text_color=_GREEN,
            ).pack(side="left", padx=8)

        # Scrollable grid
        scroll = ctk.CTkScrollableFrame(
            self.content, fg_color="transparent",
            scrollbar_button_color=_BORDER,
            scrollbar_button_hover_color=_ACCENT,
        )
        scroll.pack(fill="both", expand=True)
        for c in range(HERO_COLUMNS):
            scroll.grid_columnconfigure(c, weight=1)

        names = self.db.get_character_names()
        for i, name in enumerate(names):
            char = self.db.get_character(name)
            if not char:
                continue
            r, c = divmod(i, HERO_COLUMNS)
            swap = self.settings.get_swap(char.char_id)
            self._hero_card(scroll, r, c, char, swap)

    def _hero_card(
        self, parent, row: int, col: int,
        char: CharacterInfo, swap: Optional[SwapRecord],
    ) -> None:
        """Render a single hero card in the grid."""
        is_active = swap is not None
        card_bg = _BG_CARD_A if is_active else _BG_CARD

        card = ctk.CTkFrame(
            parent, fg_color=card_bg, corner_radius=12,
            border_width=1,
            border_color=_ACTIVE_BDG if is_active else _BORDER,
        )
        card.grid(row=row, column=col, padx=6, pady=6, sticky="nsew")
        card.configure(cursor="hand2")

        # Portrait image
        img_key = self.img_cache.hero_portrait_key(char.name)
        ctk_img = self._get_ctk_image(img_key, HERO_IMG_SIZE)
        img_label = ctk.CTkLabel(card, image=ctk_img, text="")
        img_label.pack(padx=10, pady=(12, 6))

        # Hero name
        ctk.CTkLabel(
            card, text=char.name,
            font=ctk.CTkFont(size=15, weight="bold"),
            text_color=_TEXT,
        ).pack(pady=(0, 2))

        # Active badge
        if is_active:
            ctk.CTkLabel(
                card,
                text=f"✔ {swap.skin_name}",
                font=ctk.CTkFont(size=12),
                text_color=_GREEN,
            ).pack(pady=(0, 8))
        else:
            # spacer
            ctk.CTkLabel(card, text="", height=6).pack(pady=(0, 6))

        # Click binding — bind to card and children
        def _on_click(e, n=char.name):
            self._show_skin_list(n)
        card.bind("<Button-1>", _on_click)
        for child in card.winfo_children():
            child.bind("<Button-1>", _on_click)

    # ------------------------------------------------------------------
    # Screen 2 — Skin list
    # ------------------------------------------------------------------

    def _show_skin_list(self, name: str) -> None:
        char = self.db.get_character(name)
        if not char:
            return
        self._current_char = char
        self._clear_content()

        swap = self.settings.get_swap(char.char_id)

        # Header bar
        hdr = ctk.CTkFrame(self.content, fg_color="transparent")
        hdr.pack(fill="x", pady=(0, 10))

        ctk.CTkButton(
            hdr, text="←  Back", width=90, height=32,
            fg_color="transparent", hover_color=_BG_HOVER,
            border_width=1, border_color=_BORDER,
            text_color=_TEXT,
            command=self._show_hero_grid,
        ).pack(side="left")

        ctk.CTkLabel(
            hdr, text=f"  {name}",
            font=ctk.CTkFont(size=18, weight="bold"),
            text_color=_TEXT,
        ).pack(side="left", padx=8)

        if swap:
            badge = ctk.CTkFrame(hdr, fg_color=_GREEN, corner_radius=6)
            badge.pack(side="right", padx=8)
            ctk.CTkLabel(
                badge, text=f"  ✔ {swap.skin_name}  ",
                font=ctk.CTkFont(size=12, weight="bold"),
                text_color="white",
            ).pack(padx=6, pady=3)

        # Skin grid (same layout as hero grid)
        scroll = ctk.CTkScrollableFrame(
            self.content, fg_color="transparent",
            scrollbar_button_color=_BORDER,
            scrollbar_button_hover_color=_ACCENT,
        )
        scroll.pack(fill="both", expand=True)
        for c in range(SKIN_COLUMNS):
            scroll.grid_columnconfigure(c, weight=1)

        skins = self.db.get_skins(name)
        if not skins:
            ctk.CTkLabel(
                scroll, text="No skins available.",
                font=ctk.CTkFont(size=14),
                text_color=_TEXT_DIM,
            ).grid(row=0, column=0, pady=30)
            return

        for idx, skin in enumerate(skins):
            r, c = divmod(idx, SKIN_COLUMNS)
            is_active = swap is not None and swap.skin_id == skin.skin_id
            self._skin_card(scroll, r, c, char, skin, is_active,
                            has_swap=swap is not None)

    def _skin_card(
        self, parent, row: int, col: int, char: CharacterInfo,
        skin: SkinInfo, is_active: bool, has_swap: bool,
    ) -> None:
        """Render a single skin card in the grid (mirrors hero card layout)."""
        bg = _BG_CARD_A if is_active else _BG_CARD
        card = ctk.CTkFrame(
            parent, fg_color=bg, corner_radius=12,
            border_width=1,
            border_color=_ACTIVE_BDG if is_active else _BORDER,
        )
        card.grid(row=row, column=col, padx=6, pady=6, sticky="nsew")

        # Skin thumbnail
        img_key = self.img_cache.skin_icon_key(char.name, skin.skin_id)
        ctk_img = self._get_ctk_image(img_key, SKIN_IMG_SIZE)
        ctk.CTkLabel(card, image=ctk_img, text="").pack(
            padx=10, pady=(12, 6))

        # Skin name
        ctk.CTkLabel(
            card, text=skin.skin_name,
            font=ctk.CTkFont(size=14, weight="bold" if is_active else "normal"),
            text_color=_TEXT,
        ).pack(pady=(0, 2))

        # Button area
        if is_active:
            ctk.CTkLabel(
                card, text="✔ Active",
                font=ctk.CTkFont(size=12, weight="bold"),
                text_color=_GREEN,
            ).pack(pady=(2, 4))
            ctk.CTkButton(
                card, text="Remove", width=110, height=30,
                fg_color=_RED, hover_color=_RED_H,
                corner_radius=8, text_color="white",
                command=lambda: self._on_remove(char),
            ).pack(pady=(0, 10))
        elif has_swap:
            ctk.CTkButton(
                card, text="Swap", width=110, height=30,
                fg_color=_GREY, hover_color=_GREY,
                corner_radius=8, state="disabled",
                text_color=_TEXT_DIM,
            ).pack(pady=(4, 10))
        else:
            ctk.CTkButton(
                card, text="Swap", width=110, height=30,
                fg_color=_ACCENT, hover_color=_ACCENT_H,
                corner_radius=8, text_color="#1a1a1a",
                command=lambda s=skin: self._on_swap(char, s),
            ).pack(pady=(4, 10))

    # ------------------------------------------------------------------
    # Content helpers
    # ------------------------------------------------------------------

    def _clear_content(self) -> None:
        for w in self.content.winfo_children():
            w.destroy()

    # ------------------------------------------------------------------
    # Image loading (all images preloaded at startup)
    # ------------------------------------------------------------------

    def _refresh_grid_images(self) -> None:
        """Update hero grid images in-place (no widget rebuild)."""
        if self._current_char is None:
            self._update_ctk_images_in_place()

    # ------------------------------------------------------------------
    # Swap
    # ------------------------------------------------------------------

    def _on_swap(self, character: CharacterInfo, skin: SkinInfo) -> None:
        if self._busy:
            return

        existing = self.settings.get_swap(character.char_id)
        if existing:
            self._log(
                f"❌ {character.name} already has an active swap "
                f"({existing.skin_name}). Remove it first.")
            return

        if not self.settings.game_paks_dir:
            self._log("❌ Configure Game Paks Folder in Settings first.")
            return

        self._busy = True
        self._clear_log()
        self._show_log()
        self._log(f"▶ Swapping {character.name} → {skin.skin_name} "
                  f"({skin.skin_id})")

        self._show_skin_list(character.name)

        threading.Thread(
            target=self._run_swap,
            args=(character, skin),
            daemon=True,
        ).start()

    def _run_swap(self, character: CharacterInfo, skin: SkinInfo) -> None:
        try:
            game_paks = Path(self.settings.game_paks_dir)
            if not game_paks.is_dir():
                self._log_async("❌ Game Paks Folder not found.")
                return

            retoc = RetocWrapper(output_dir=self.output_dir,
                                  game_paks_dir=game_paks)
            for p in retoc.validate():
                self._log_async(f"⚠️  {p}")
                return

            uassettool = UAssetToolWrapper(output_dir=self.output_dir)
            for p in uassettool.validate():
                self._log_async(f"⚠️  {p}")
                return

            engine = SwapEngine(retoc, uassettool)
            res = engine.create_skin_swap(
                character=character,
                source_skin=skin,
                log_callback=lambda m: self._log_async(f"  {m}"),
            )

            if not res.success:
                self._log_async(f"❌ Swap failed: {res.error}")
                return

            pr = res.pack_result
            record = SwapRecord(
                skin_id=skin.skin_id,
                skin_name=skin.skin_name,
                mod_name=res.mod_name,
                pak_path=str(pr.pak_path) if pr.pak_path else "",
                utoc_path=str(pr.utoc_path) if pr.utoc_path else "",
                ucas_path=str(pr.ucas_path) if pr.ucas_path else "",
            )

            if self.settings.auto_deploy:
                self._log_async("🚀 Deploying to ~mods...")
                dr = retoc.deploy_to_mods(res.pack_result)
                if dr.success:
                    self._log_async(f"✅ Deployed to {game_paks / '~mods'}")
                    record.pak_path  = str(dr.pak_path)  if dr.pak_path  else ""
                    record.utoc_path = str(dr.utoc_path) if dr.utoc_path else ""
                    record.ucas_path = str(dr.ucas_path) if dr.ucas_path else ""
                else:
                    self._log_async(f"⚠️  Deploy failed: {dr.error}")

            self.settings.set_swap(character.char_id, record)
            save_settings(self.settings)
            self._log_async(f"🎉 Done! {res.files_created} files swapped")

        except Exception as exc:
            self._log_async(f"❌ Unexpected error: {exc}")
        finally:
            self._busy = False
            self.after(0, lambda: self._show_skin_list(character.name))

    # ------------------------------------------------------------------
    # Remove swap
    # ------------------------------------------------------------------

    def _on_remove(self, character: CharacterInfo) -> None:
        if self._busy:
            return

        swap = self.settings.get_swap(character.char_id)
        if not swap:
            self._log("Nothing to remove.")
            return

        self._clear_log()
        self._show_log()
        self._log(f"🗑️  Removing {character.name} swap ({swap.skin_name})...")

        removed = 0
        for p_str in (swap.pak_path, swap.utoc_path, swap.ucas_path):
            if p_str:
                p = Path(p_str)
                if p.exists():
                    try:
                        p.unlink()
                        removed += 1
                    except OSError as e:
                        self._log(f"  ⚠️  Could not delete {p.name}: {e}")

        if swap.mod_name:
            for ext in (".pak", ".utoc", ".ucas"):
                p = self.output_dir / f"{swap.mod_name}_9999999_P{ext}"
                if p.exists():
                    try:
                        p.unlink()
                        removed += 1
                    except OSError:
                        pass

        self.settings.clear_swap(character.char_id)
        save_settings(self.settings)

        self._log(f"✅ Removed — {removed} file(s) deleted")
        self._show_skin_list(character.name)

    # ------------------------------------------------------------------
    # Settings
    # ------------------------------------------------------------------

    def _open_settings(self) -> None:
        SettingsWindow(self, self.settings, on_save=self._on_settings_saved)

    def _on_settings_saved(self) -> None:
        self._log("Settings saved.")

    # ------------------------------------------------------------------
    # Logging
    # ------------------------------------------------------------------

    def _log(self, msg: str) -> None:
        self.log_box.configure(state="normal")
        self.log_box.insert("end", msg + "\n")
        self.log_box.see("end")
        self.log_box.configure(state="disabled")

    def _log_async(self, msg: str) -> None:
        self.after(0, lambda: self._log(msg))

    def _clear_log(self) -> None:
        self.log_box.configure(state="normal")
        self.log_box.delete("1.0", "end")
        self.log_box.configure(state="disabled")
