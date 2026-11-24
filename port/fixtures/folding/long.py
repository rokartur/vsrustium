"""Inventory helpers for the folding pixel check: enough functions to scroll past one screen."""

import json
from dataclasses import dataclass, field


@dataclass
class Item:
    name: str
    quantity: int = 0
    tags: list[str] = field(default_factory=list)

    def restock(self, amount):
        if amount <= 0:
            raise ValueError("amount must be positive")
        self.quantity += amount
        return self.quantity

    def take(self, amount):
        if amount > self.quantity:
            return False
        self.quantity -= amount
        return True


class Inventory:
    def __init__(self):
        self.items = {}

    def add(self, item):
        self.items[item.name] = item

    def find(self, name):
        return self.items.get(name)

    def low_stock(self, threshold=5):
        result = []
        for item in self.items.values():
            if item.quantity < threshold:
                result.append(item)
        return result

    def tagged(self, tag):
        return [item for item in self.items.values() if tag in item.tags]


#region Serialization
def to_json(inventory):
    data = []
    for item in inventory.items.values():
        data.append({
            "name": item.name,
            "quantity": item.quantity,
            "tags": item.tags,
        })
    return json.dumps(data, indent=2)


def from_json(text):
    inventory = Inventory()
    for entry in json.loads(text):
        inventory.add(Item(entry["name"], entry["quantity"], entry["tags"]))
    return inventory
#endregion


def report(inventory):
    lines = []
    for item in sorted(inventory.items.values(), key=lambda item: item.name):
        marker = "!" if item.quantity < 5 else " "
        lines.append(f"{marker} {item.name:<20} {item.quantity:>5}")
    return "\n".join(lines)


def merge(first, second):
    merged = Inventory()
    for source in (first, second):
        for item in source.items.values():
            existing = merged.find(item.name)
            if existing is None:
                merged.add(Item(item.name, item.quantity, list(item.tags)))
            else:
                existing.restock(item.quantity)
                for tag in item.tags:
                    if tag not in existing.tags:
                        existing.tags.append(tag)
    return merged


def main():
    inventory = Inventory()
    inventory.add(Item("bolts", 120, ["hardware"]))
    inventory.add(Item("nuts", 3, ["hardware"]))
    inventory.add(Item("glue", 1, ["supplies"]))
    print(report(inventory))
    print(to_json(inventory))


if __name__ == "__main__":
    main()
