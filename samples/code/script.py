"""Sample Python module — syntax highlighting demo."""
from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class User:
    name: str
    age: int = 0
    tags: list[str] = field(default_factory=list)

    def greet(self) -> str:
        return f"Hello, {self.name}!"  # f-string


def total_age(users: list[User]) -> int:
    return sum(u.age for u in users)


def main() -> None:
    users = [User("Alice", 30, ["admin"]), User("Bob")]
    for u in users:
        print(u.greet())
    assert total_age(users) == 30, "unexpected total"


if __name__ == "__main__":
    main()
