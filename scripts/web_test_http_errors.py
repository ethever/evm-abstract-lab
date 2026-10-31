"""Consume only explicitly injected, response-confirmed HTTP console failures."""

from dataclasses import dataclass
import re


@dataclass
class ExpectedHttpError:
    method: str
    url: str
    status: int
    responses: int = 0
    console_messages: int = 0

    def assert_complete(self):
        assert self.responses == self.console_messages == 1, self.evidence()

    def evidence(self):
        return {"method": self.method, "url": self.url, "status": self.status,
                "responses": self.responses, "console_messages": self.console_messages}


class ExpectedHttpErrors:
    def __init__(self):
        self.expected = []

    def expect(self, method, url, status):
        assert method and method == method.upper()
        assert url.startswith(("http://", "https://"))
        assert type(status) is int and 400 <= status <= 599
        assert not any((item.method, item.url, item.status) == (method, url, status)
                       for item in self.expected), "duplicate HTTP-error expectation"
        item = ExpectedHttpError(method, url, status)
        self.expected.append(item)
        return item

    def response(self, method, url, status):
        for item in self.expected:
            if (item.method, item.url, item.status) == (method, url, status):
                item.responses += 1

    def console(self, url, text):
        match = re.fullmatch(r"Failed to load resource: the server responded with a status of (\d+) \([^\n]*\)", text)
        if match is None:
            return False
        for item in self.expected:
            if item.url == url and item.status == int(match[1]) and item.console_messages == 0:
                item.console_messages += 1
                return True
        return False

    def assert_complete(self):
        for item in self.expected:
            item.assert_complete()
