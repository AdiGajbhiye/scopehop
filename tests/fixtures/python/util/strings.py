PREFIX = "!"


def shout(s: str) -> str:
    return s.upper() + PREFIX


def count_words(text):
    words = text.split()
    total = 0
    for w in words:
        total += 1
    return total
