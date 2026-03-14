from typing import List


def quicksort(arr: List[int]) -> List[int]:
    if len(arr) <= 1:
        return arr

    pivot = arr[len(arr) // 2]
    left = [x for x in arr if x < pivot]
    middle = [x for x in arr if x == pivot]
    right = [x for x in arr if x > pivot]

    return quicksort(left) + middle + quicksort(right)


if __name__ == "__main__":
    nums = [10, 7, 8, 9, 1, 5, 3, 7]
    print("原始数组:", nums)
    print("排序结果:", quicksort(nums))
