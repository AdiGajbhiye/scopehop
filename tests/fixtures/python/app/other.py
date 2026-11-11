from models.shapes import Circle as Round


class Config:
    debug = False


def helper(config: Config) -> Round:
    if config.debug:
        return Round(1.0)
    return Round.unit()
