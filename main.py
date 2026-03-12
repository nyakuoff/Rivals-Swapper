"""
MR-SkinChanger — Marvel Rivals Skin Changer

Entry point.  Run with:
    python main.py
"""

from src.gui import App


def main() -> None:
    app = App()
    app.mainloop()


if __name__ == "__main__":
    main()
